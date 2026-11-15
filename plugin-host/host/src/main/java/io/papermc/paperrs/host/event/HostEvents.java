package io.papermc.paperrs.host.event;

import com.destroystokyo.paper.event.server.PaperServerListPingEvent;
import com.destroystokyo.paper.network.StatusClient;
import io.papermc.paper.chat.ChatRenderer;
import io.papermc.paper.event.player.AsyncChatEvent;
import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.entity.HostPlayer;
import io.papermc.paperrs.host.world.HostBlock;
import io.papermc.paperrs.host.world.HostBlockState;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.SocketAddress;
import java.time.Instant;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import net.kyori.adventure.audience.Audience;
import net.kyori.adventure.chat.SignedMessage;
import net.kyori.adventure.identity.Identity;
import net.kyori.adventure.text.Component;
import org.bukkit.GameMode;
import org.bukkit.Location;
import org.bukkit.World;
import org.bukkit.block.Block;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.Player;
import org.bukkit.event.Event;
import org.bukkit.event.HandlerList;
import org.bukkit.event.block.BlockBreakEvent;
import org.bukkit.event.block.BlockPlaceEvent;
import org.bukkit.event.player.AsyncPlayerChatEvent;
import org.bukkit.event.player.AsyncPlayerPreLoginEvent;
import org.bukkit.event.player.PlayerCommandPreprocessEvent;
import org.bukkit.event.player.PlayerGameModeChangeEvent;
import org.bukkit.event.player.PlayerJoinEvent;
import org.bukkit.event.player.PlayerMoveEvent;
import org.bukkit.event.player.PlayerQuitEvent;
import org.bukkit.event.server.ServerCommandEvent;
import org.bukkit.inventory.EquipmentSlot;
import org.jetbrains.annotations.Nullable;

/** Fires Bukkit and Paper events for the {@code event.*} calls of the Rust server. */
public final class HostEvents {
    /**
     * Events the server only fires when some plugin listens, keyed by the simple class name
     * the server uses.
     */
    private static final Map<String, Class<? extends Event>> GATED = new LinkedHashMap<>();

    static {
        GATED.put("PlayerCommandPreprocessEvent", PlayerCommandPreprocessEvent.class);
        GATED.put("ServerCommandEvent", ServerCommandEvent.class);
        GATED.put("BlockBreakEvent", BlockBreakEvent.class);
        GATED.put("PlayerResourcePackStatusEvent", org.bukkit.event.player.PlayerResourcePackStatusEvent.class);
        GATED.put("BlockPlaceEvent", BlockPlaceEvent.class);
        GATED.put("PlayerMoveEvent", PlayerMoveEvent.class);
        GATED.put("PlayerGameModeChangeEvent", PlayerGameModeChangeEvent.class);
        GATED.put("AsyncChatEvent", AsyncChatEvent.class);
        GATED.put("AsyncPlayerChatEvent", AsyncPlayerChatEvent.class);
        GATED.put("InventoryOpenEvent", org.bukkit.event.inventory.InventoryOpenEvent.class);
        GATED.put("InventoryClickEvent", org.bukkit.event.inventory.InventoryClickEvent.class);
        GATED.put("InventoryDragEvent", org.bukkit.event.inventory.InventoryDragEvent.class);
        GATED.put("InventoryCloseEvent", org.bukkit.event.inventory.InventoryCloseEvent.class);
        GATED.put("CraftItemEvent", org.bukkit.event.inventory.CraftItemEvent.class);
        GATED.put("EntityPickupItemEvent", org.bukkit.event.entity.EntityPickupItemEvent.class);
        GATED.put("PlayerAttemptPickupItemEvent", org.bukkit.event.player.PlayerAttemptPickupItemEvent.class);
        GATED.put("PlayerPickupItemEvent", org.bukkit.event.player.PlayerPickupItemEvent.class);
        GATED.put("PlayerDropItemEvent", org.bukkit.event.player.PlayerDropItemEvent.class);
        GATED.put("EntityDamageEvent", org.bukkit.event.entity.EntityDamageEvent.class);
        GATED.put("EntityDamageByEntityEvent", org.bukkit.event.entity.EntityDamageByEntityEvent.class);
        GATED.put("EntityDamageByBlockEvent", org.bukkit.event.entity.EntityDamageByBlockEvent.class);
        GATED.put("EntityDeathEvent", org.bukkit.event.entity.EntityDeathEvent.class);
        GATED.put("PlayerDeathEvent", org.bukkit.event.entity.PlayerDeathEvent.class);
        GATED.put("PlayerRespawnEvent", org.bukkit.event.player.PlayerRespawnEvent.class);
        GATED.put("CreatureSpawnEvent", org.bukkit.event.entity.CreatureSpawnEvent.class);
        // PlayerTradeEvent shares PlayerPurchaseEvent's handler list.
        GATED.put("PlayerPurchaseEvent", io.papermc.paper.event.player.PlayerPurchaseEvent.class);
        GATED.put("TradeSelectEvent", org.bukkit.event.inventory.TradeSelectEvent.class);
        GATED.put("VillagerAcquireTradeEvent", org.bukkit.event.entity.VillagerAcquireTradeEvent.class);
        GATED.put("VillagerCareerChangeEvent", org.bukkit.event.entity.VillagerCareerChangeEvent.class);
        GATED.put("VillagerReplenishTradeEvent", org.bukkit.event.entity.VillagerReplenishTradeEvent.class);
        GATED.put("EntityMountEvent", org.bukkit.event.entity.EntityMountEvent.class);
        GATED.put("EntityDismountEvent", org.bukkit.event.entity.EntityDismountEvent.class);
        GATED.put("EntityTameEvent", org.bukkit.event.entity.EntityTameEvent.class);
        GATED.put("PlayerStatisticIncrementEvent", org.bukkit.event.player.PlayerStatisticIncrementEvent.class);
        GATED.put("SignChangeEvent", org.bukkit.event.block.SignChangeEvent.class);
        GATED.put("PlayerInteractEvent", org.bukkit.event.player.PlayerInteractEvent.class);
        GATED.put("PlayerInteractEntityEvent", org.bukkit.event.player.PlayerInteractEntityEvent.class);
        GATED.put("EntityExplodeEvent", org.bukkit.event.entity.EntityExplodeEvent.class);
        GATED.put("BlockExplodeEvent", org.bukkit.event.block.BlockExplodeEvent.class);
        GATED.put("PlayerToggleSneakEvent", org.bukkit.event.player.PlayerToggleSneakEvent.class);
        GATED.put("PlayerToggleSprintEvent", org.bukkit.event.player.PlayerToggleSprintEvent.class);
        GATED.put("PlayerToggleFlightEvent", org.bukkit.event.player.PlayerToggleFlightEvent.class);
        GATED.put("PlayerItemHeldEvent", org.bukkit.event.player.PlayerItemHeldEvent.class);
        GATED.put("PlayerSwapHandItemsEvent", org.bukkit.event.player.PlayerSwapHandItemsEvent.class);
        GATED.put("PlayerItemConsumeEvent", org.bukkit.event.player.PlayerItemConsumeEvent.class);
        GATED.put("FoodLevelChangeEvent", org.bukkit.event.entity.FoodLevelChangeEvent.class);
        GATED.put("PlayerBucketEmptyEvent", org.bukkit.event.player.PlayerBucketEmptyEvent.class);
        GATED.put("PlayerBucketFillEvent", org.bukkit.event.player.PlayerBucketFillEvent.class);
        GATED.put("WeatherChangeEvent", org.bukkit.event.weather.WeatherChangeEvent.class);
        GATED.put("ThunderChangeEvent", org.bukkit.event.weather.ThunderChangeEvent.class);
        // Teleports the server makes (plugins' own are fired where they ask for them).
        GATED.put("PlayerTeleportEvent", org.bukkit.event.player.PlayerTeleportEvent.class);
        GATED.put("PlayerPortalEvent", org.bukkit.event.player.PlayerPortalEvent.class);
        // (EntityTargetLivingEntityEvent shares its handler list.)
        GATED.put("EntityTargetEvent", org.bukkit.event.entity.EntityTargetEvent.class);
        // Piston moves and fire wait in the block engine while these have listeners.
        GATED.put("BlockPistonExtendEvent", org.bukkit.event.block.BlockPistonExtendEvent.class);
        GATED.put("BlockPistonRetractEvent", org.bukkit.event.block.BlockPistonRetractEvent.class);
        GATED.put("BlockBurnEvent", org.bukkit.event.block.BlockBurnEvent.class);
        GATED.put("BlockIgniteEvent", org.bukkit.event.block.BlockIgniteEvent.class);
        GATED.put("BlockSpreadEvent", org.bukkit.event.block.BlockSpreadEvent.class);
        GATED.put("BlockFromToEvent", org.bukkit.event.block.BlockFromToEvent.class);
        GATED.put("BlockDamageEvent", org.bukkit.event.block.BlockDamageEvent.class);
        GATED.put("StructureGrowEvent", org.bukkit.event.world.StructureGrowEvent.class);
        GATED.put("BlockFertilizeEvent", org.bukkit.event.block.BlockFertilizeEvent.class);
        GATED.put("PortalCreateEvent", org.bukkit.event.world.PortalCreateEvent.class);
        // WaterBottleSplashEvent shares PotionSplashEvent's listeners.
        GATED.put("PotionSplashEvent", org.bukkit.event.entity.PotionSplashEvent.class);
        GATED.put("LingeringPotionSplashEvent", org.bukkit.event.entity.LingeringPotionSplashEvent.class);
        GATED.put("AreaEffectCloudApplyEvent", org.bukkit.event.entity.AreaEffectCloudApplyEvent.class);
        GATED.put("PlayerItemDamageEvent", org.bukkit.event.player.PlayerItemDamageEvent.class);
        GATED.put("PlayerItemBreakEvent", org.bukkit.event.player.PlayerItemBreakEvent.class);
        GATED.put("PlayerShearEntityEvent", org.bukkit.event.player.PlayerShearEntityEvent.class);
        GATED.put("BlockShearEntityEvent", org.bukkit.event.block.BlockShearEntityEvent.class);
        GATED.put("PlayerBedEnterEvent", org.bukkit.event.player.PlayerBedEnterEvent.class);
        GATED.put("PlayerBedLeaveEvent", org.bukkit.event.player.PlayerBedLeaveEvent.class);
        GATED.put("PlayerChangedMainHandEvent", org.bukkit.event.player.PlayerChangedMainHandEvent.class);
        GATED.put("PlayerLocaleChangeEvent", org.bukkit.event.player.PlayerLocaleChangeEvent.class);
        GATED.put("PlayerPickupArrowEvent", org.bukkit.event.player.PlayerPickupArrowEvent.class);
        GATED.put("PlayerEggThrowEvent", org.bukkit.event.player.PlayerEggThrowEvent.class);
        GATED.put("ThrownEggHatchEvent", com.destroystokyo.paper.event.entity.ThrownEggHatchEvent.class);
        GATED.put("ExpBottleEvent", org.bukkit.event.entity.ExpBottleEvent.class);
        GATED.put("ItemDespawnEvent", org.bukkit.event.entity.ItemDespawnEvent.class);
        GATED.put("ItemMergeEvent", org.bukkit.event.entity.ItemMergeEvent.class);
        GATED.put("TNTPrimeEvent", org.bukkit.event.block.TNTPrimeEvent.class);
        GATED.put("ExplosionPrimeEvent", org.bukkit.event.entity.ExplosionPrimeEvent.class);
        GATED.put("LightningStrikeEvent", org.bukkit.event.weather.LightningStrikeEvent.class);
        GATED.put("CreeperPowerEvent", org.bukkit.event.entity.CreeperPowerEvent.class);
        GATED.put("PigZapEvent", org.bukkit.event.entity.PigZapEvent.class);
        GATED.put("EntityZapEvent", com.destroystokyo.paper.event.entity.EntityZapEvent.class);
        GATED.put("EntityTransformEvent", org.bukkit.event.entity.EntityTransformEvent.class);
        GATED.put("VehicleEnterEvent", org.bukkit.event.vehicle.VehicleEnterEvent.class);
        GATED.put("VehicleExitEvent", org.bukkit.event.vehicle.VehicleExitEvent.class);
        GATED.put("VehicleDamageEvent", org.bukkit.event.vehicle.VehicleDamageEvent.class);
        GATED.put("VehicleDestroyEvent", org.bukkit.event.vehicle.VehicleDestroyEvent.class);
        GATED.put("VehicleCreateEvent", org.bukkit.event.vehicle.VehicleCreateEvent.class);
        GATED.put("VehicleMoveEvent", org.bukkit.event.vehicle.VehicleMoveEvent.class);
        GATED.put("EntityPlaceEvent", org.bukkit.event.entity.EntityPlaceEvent.class);
        GATED.put("ProjectileLaunchEvent", org.bukkit.event.entity.ProjectileLaunchEvent.class);
        GATED.put("ProjectileHitEvent", org.bukkit.event.entity.ProjectileHitEvent.class);
        GATED.put("BlockDispenseEvent", org.bukkit.event.block.BlockDispenseEvent.class);
        GATED.put("PlayerKickEvent", org.bukkit.event.player.PlayerKickEvent.class);
        GATED.put("EntityRegainHealthEvent", org.bukkit.event.entity.EntityRegainHealthEvent.class);
        // (The ByEntity and ByBlock forms share its handler list.)
        GATED.put("EntityCombustEvent", org.bukkit.event.entity.EntityCombustEvent.class);
        GATED.put("EntityChangeBlockEvent", org.bukkit.event.entity.EntityChangeBlockEvent.class);
        GATED.put("BlockGrowEvent", org.bukkit.event.block.BlockGrowEvent.class);
        GATED.put("BlockFadeEvent", org.bukkit.event.block.BlockFadeEvent.class);
        GATED.put("BlockFormEvent", org.bukkit.event.block.BlockFormEvent.class);
        GATED.put("LeavesDecayEvent", org.bukkit.event.block.LeavesDecayEvent.class);
        GATED.put("MoistureChangeEvent", org.bukkit.event.block.MoistureChangeEvent.class);
        GATED.put("PlayerChangedWorldEvent", org.bukkit.event.player.PlayerChangedWorldEvent.class);
        GATED.put("PlayerLoginEvent", org.bukkit.event.player.PlayerLoginEvent.class);
        GATED.put("ChunkLoadEvent", org.bukkit.event.world.ChunkLoadEvent.class);
        GATED.put("ChunkUnloadEvent", org.bukkit.event.world.ChunkUnloadEvent.class);
        GATED.put("EntityPotionEffectEvent", org.bukkit.event.entity.EntityPotionEffectEvent.class);
        GATED.put("EntityShootBowEvent", org.bukkit.event.entity.EntityShootBowEvent.class);
        GATED.put("ItemSpawnEvent", org.bukkit.event.entity.ItemSpawnEvent.class);
        GATED.put("SpawnerSpawnEvent", org.bukkit.event.entity.SpawnerSpawnEvent.class);
        GATED.put("PlayerOpenSignEvent", io.papermc.paper.event.player.PlayerOpenSignEvent.class);
        GATED.put("PlayerSignOpenEvent", org.bukkit.event.player.PlayerSignOpenEvent.class);
        GATED.put("UnknownCommandEvent", org.bukkit.event.command.UnknownCommandEvent.class);
        GATED.put("PlayerClientLoadedWorldEvent", io.papermc.paper.event.player.PlayerClientLoadedWorldEvent.class);
        GATED.put("BlockPreDispenseEvent", io.papermc.paper.event.block.BlockPreDispenseEvent.class);
        GATED.put("BlockFailedDispenseEvent", io.papermc.paper.event.block.BlockFailedDispenseEvent.class);
    }

    private final CraftServer server;
    private final Map<String, HandlerList> handlerLists = new LinkedHashMap<>();
    private final InventoryEvents inventories;
    private final EntityEvents entities;
    private final MerchantEvents merchants;

    public HostEvents(final CraftServer server) {
        this.server = server;
        this.inventories = new InventoryEvents(server);
        this.entities = new EntityEvents(server);
        this.merchants = new MerchantEvents(server);
        for (Map.Entry<String, Class<? extends Event>> e : GATED.entrySet()) {
            try {
                this.handlerLists.put(e.getKey(), (HandlerList) e.getValue().getMethod("getHandlerList").invoke(null));
            } catch (ReflectiveOperationException ex) {
                throw new IllegalStateException(e.getKey() + " has no handler list", ex);
            }
        }
    }

    /** The gated events that currently have listeners. */
    public Set<String> listened() {
        final Set<String> out = new LinkedHashSet<>();
        this.handlerLists.forEach((name, list) -> {
            final org.bukkit.plugin.RegisteredListener[] all = list.getRegisteredListeners();
            if (all.length > 0 && (!NARROWED.contains(name) || this.heard(name, all))) {
                out.add(name);
            }
        });
        return out;
    }

    /**
     * Events whose handler list is another's: {@code EntitySpawnEvent}'s also holds the
     * listeners of creature, item and spawner spawns. Each of these counts as listened only
     * when some listener takes it or a supertype, so that a creature spawn listener does not
     * make every dropped item wait for the host.
     */
    private static final Set<String> NARROWED = Set.of("CreatureSpawnEvent", "ItemSpawnEvent", "SpawnerSpawnEvent");
    /** For each narrowed event, the listeners last looked at (lists keep their array until they change) and the answer. */
    private final Map<String, org.bukkit.plugin.RegisteredListener[]> narrowedFor = new java.util.HashMap<>();
    private final Map<String, Boolean> narrowedHeard = new java.util.HashMap<>();

    private boolean heard(final String name, final org.bukkit.plugin.RegisteredListener[] all) {
        if (this.narrowedFor.get(name) != all) {
            final Class<? extends Event> type = GATED.get(name);
            boolean heard = false;
            for (final org.bukkit.plugin.RegisteredListener listener : all) {
                if (takes(listener.getListener().getClass(), type)) {
                    heard = true;
                    break;
                }
            }
            this.narrowedFor.put(name, all);
            this.narrowedHeard.put(name, heard);
        }
        return this.narrowedHeard.get(name);
    }

    /**
     * The event types that a listener class's {@code @EventHandler} methods take, found as
     * Paper's event manager finds them. A class without any was registered some other way
     * ({@code PluginManager#registerEvent}) and might take anything.
     */
    private static final ClassValue<List<Class<?>>> HANDLED = new ClassValue<>() {
        @Override
        protected List<Class<?>> computeValue(final Class<?> type) {
            final Set<java.lang.reflect.Method> methods = new java.util.HashSet<>();
            try {
                methods.addAll(List.of(type.getMethods()));
                methods.addAll(List.of(type.getDeclaredMethods()));
            } catch (final LinkageError e) {
                return List.of(Event.class);
            }
            final List<Class<?>> out = new ArrayList<>();
            for (final java.lang.reflect.Method method : methods) {
                if (method.isAnnotationPresent(org.bukkit.event.EventHandler.class) && method.getParameterCount() == 1) {
                    out.add(method.getParameterTypes()[0]);
                }
            }
            return out.isEmpty() ? List.of(Event.class) : List.copyOf(out);
        }
    };

    private static boolean takes(final Class<?> listener, final Class<? extends Event> type) {
        for (final Class<?> handled : HANDLED.get(listener)) {
            if (handled.isAssignableFrom(type)) {
                return true;
            }
        }
        return false;
    }

    /** The faces of the protocol's directions, 0 down to 5 east. */
    private static final org.bukkit.block.BlockFace[] FACES = {
        org.bukkit.block.BlockFace.DOWN, org.bukkit.block.BlockFace.UP, org.bukkit.block.BlockFace.NORTH,
        org.bukkit.block.BlockFace.SOUTH, org.bukkit.block.BlockFace.WEST, org.bukkit.block.BlockFace.EAST
    };

    /**
     * {@code PlayerInteractEvent}, as CraftEventFactory makes it: no item for an empty hand, no
     * hand for a physical interaction, the clicked face ({@code SELF} for none) and where on
     * the block. Clicks in the air start out with the block's use denied (so cancelled).
     */
    private Map<String, Object> interact(final Map<String, Object> args) {
        final HostPlayer player = this.player(args);
        final org.bukkit.event.block.Action action = org.bukkit.event.block.Action.valueOf(Values.str(args, "action"));
        final EquipmentSlot hand = action == org.bukkit.event.block.Action.PHYSICAL ? null : Values.i32(args, "hand", 0) == 1 ? EquipmentSlot.OFF_HAND : EquipmentSlot.HAND;
        final org.bukkit.inventory.ItemStack held = hand == null ? null : player.getInventory().getItem(hand);
        final Block block = args.containsKey("x") ? this.block(args) : null;
        final int face = Values.i32(args, "face", -1);
        final List<Object> cursor = Values.list(args.get("cursor"));
        final org.bukkit.util.Vector clicked = cursor.size() == 3
            ? new org.bukkit.util.Vector(((Number) cursor.get(0)).doubleValue(), ((Number) cursor.get(1)).doubleValue(), ((Number) cursor.get(2)).doubleValue()) : null;
        final org.bukkit.event.player.PlayerInteractEvent event = new org.bukkit.event.player.PlayerInteractEvent(player, action,
            held == null || held.isEmpty() ? null : held, block, face >= 0 && face < FACES.length ? FACES[face] : org.bukkit.block.BlockFace.SELF, hand, clicked);
        event.callEvent();
        return Values.of("use_block", event.useInteractedBlock().name(), "use_item", event.useItemInHand().name());
    }

    /**
     * {@code EntityExplodeEvent}, or {@code BlockExplodeEvent} for an explosion no entity
     * caused: the blocks it is to break, as plugins leave them.
     */
    private Map<String, Object> explode(final Map<String, Object> args) {
        final World world = this.server.getWorld(Values.str(args, "world"));
        final Location at = new Location(world, Values.f64(args, "x", 0), Values.f64(args, "y", 0), Values.f64(args, "z", 0));
        final List<Block> blocks = new ArrayList<>();
        for (final Object b : Values.list(args.get("blocks"))) {
            final List<Object> xyz = Values.list(b);
            blocks.add(world.getBlockAt(((Number) xyz.get(0)).intValue(), ((Number) xyz.get(1)).intValue(), ((Number) xyz.get(2)).intValue()));
        }
        final org.bukkit.ExplosionResult result = org.bukkit.ExplosionResult.valueOf(Values.str(args, "result", "DESTROY"));
        final Object source = args.get("entity");
        final org.bukkit.entity.Entity entity = source instanceof Map<?, ?> ref ? io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(ref)) : null;
        final boolean cancelled;
        final List<Block> left;
        if (entity != null) {
            final org.bukkit.event.entity.EntityExplodeEvent event = new org.bukkit.event.entity.EntityExplodeEvent(entity, at, blocks, 1.0f, result);
            event.callEvent();
            cancelled = event.isCancelled();
            left = event.blockList();
        } else {
            final Block block = at.getBlock();
            final org.bukkit.event.block.BlockExplodeEvent event = new org.bukkit.event.block.BlockExplodeEvent(block, block.getState(), blocks, 1.0f, result);
            event.callEvent();
            cancelled = event.isCancelled();
            left = event.blockList();
        }
        final List<Object> out = new ArrayList<>();
        for (final Block b : left) {
            if (b.getWorld().equals(world)) {
                out.add(List.of((long) b.getX(), (long) b.getY(), (long) b.getZ()));
            }
        }
        return Values.of("cancelled", cancelled, "blocks", out);
    }

    /**
     * {@code PlayerBucketEmptyEvent} / {@code PlayerBucketFillEvent}: the block the liquid goes
     * to (or comes from), the clicked block and face, the bucket and what it becomes.
     */
    private Map<String, Object> bucket(final Map<String, Object> args, final boolean fill) {
        final HostPlayer player = this.player(args);
        final Block block = this.block(args);
        final List<Object> at = Values.list(args.get("clicked"));
        final Block clicked = block.getWorld().getBlockAt(((Number) at.get(0)).intValue(), ((Number) at.get(1)).intValue(), ((Number) at.get(2)).intValue());
        final int face = Values.i32(args, "face", 1);
        final org.bukkit.block.BlockFace blockFace = face >= 0 && face < FACES.length ? FACES[face] : org.bukkit.block.BlockFace.SELF;
        final org.bukkit.Material bucket = org.bukkit.Registry.MATERIAL.getOrThrow(org.bukkit.NamespacedKey.fromString(Values.str(args, "bucket")));
        final org.bukkit.inventory.ItemStack item = io.papermc.paperrs.host.item.HostItemStack.fromValue(args.get("item"));
        final EquipmentSlot hand = Values.i32(args, "hand", 0) == 1 ? EquipmentSlot.OFF_HAND : EquipmentSlot.HAND;
        final org.bukkit.event.player.PlayerBucketEvent event = fill
            ? new org.bukkit.event.player.PlayerBucketFillEvent(player, block, clicked, blockFace, bucket, item, hand)
            : new org.bukkit.event.player.PlayerBucketEmptyEvent(player, block, clicked, blockFace, bucket, item, hand);
        event.callEvent();
        final org.bukkit.inventory.ItemStack result = event.getItemStack();
        return Values.of("cancelled", event.isCancelled(), "item", result == null || result.equals(item) ? null : io.papermc.paperrs.host.item.HostItemStack.toValue(result));
    }

    private HostPlayer player(final Map<String, Object> args) {
        final HostPlayer player = this.server.player(Values.uuid(args, "uuid"));
        if (player == null) {
            throw new IllegalArgumentException("player " + args.get("uuid") + " is not online");
        }
        return player;
    }

    /** A location with its world given by name. */
    private Location worldLocation(final Map<String, Object> loc) {
        return this.location(this.server.getWorld(Values.str(loc, "world")), loc);
    }

    private Location location(final World world, final Map<String, Object> loc) {
        return new Location(world, Values.f64(loc, "x", 0), Values.f64(loc, "y", 0), Values.f64(loc, "z", 0), (float) Values.f64(loc, "yaw", 0), (float) Values.f64(loc, "pitch", 0));
    }

    /** Paper's {@code CraftTreeType.fromFeature}: the TreeType of a grown feature. */
    private static final Map<String, org.bukkit.TreeType> TREE_TYPES = Map.ofEntries(
        Map.entry("oak", org.bukkit.TreeType.TREE), Map.entry("oak_bees_005", org.bukkit.TreeType.TREE),
        Map.entry("fancy_oak", org.bukkit.TreeType.BIG_TREE), Map.entry("fancy_oak_bees_005", org.bukkit.TreeType.BIG_TREE),
        Map.entry("spruce", org.bukkit.TreeType.REDWOOD), Map.entry("pine", org.bukkit.TreeType.TALL_REDWOOD),
        Map.entry("mega_spruce", org.bukkit.TreeType.MEGA_REDWOOD), Map.entry("mega_pine", org.bukkit.TreeType.MEGA_PINE),
        Map.entry("birch", org.bukkit.TreeType.BIRCH), Map.entry("birch_bees_005", org.bukkit.TreeType.BIRCH),
        Map.entry("super_birch_bees_0002", org.bukkit.TreeType.TALL_BIRCH),
        Map.entry("mega_jungle_tree", org.bukkit.TreeType.JUNGLE), Map.entry("jungle_tree_no_vine", org.bukkit.TreeType.SMALL_JUNGLE),
        Map.entry("jungle_tree", org.bukkit.TreeType.COCOA_TREE), Map.entry("jungle_bush", org.bukkit.TreeType.JUNGLE_BUSH),
        Map.entry("huge_red_mushroom", org.bukkit.TreeType.RED_MUSHROOM), Map.entry("huge_brown_mushroom", org.bukkit.TreeType.BROWN_MUSHROOM),
        Map.entry("swamp_oak", org.bukkit.TreeType.SWAMP), Map.entry("acacia", org.bukkit.TreeType.ACACIA), Map.entry("dark_oak", org.bukkit.TreeType.DARK_OAK),
        Map.entry("crimson_fungus", org.bukkit.TreeType.CRIMSON_FUNGUS), Map.entry("crimson_fungus_planted", org.bukkit.TreeType.CRIMSON_FUNGUS),
        Map.entry("warped_fungus", org.bukkit.TreeType.WARPED_FUNGUS), Map.entry("warped_fungus_planted", org.bukkit.TreeType.WARPED_FUNGUS),
        Map.entry("azalea_tree", org.bukkit.TreeType.AZALEA), Map.entry("mangrove", org.bukkit.TreeType.MANGROVE),
        Map.entry("tall_mangrove", org.bukkit.TreeType.TALL_MANGROVE), Map.entry("cherry", org.bukkit.TreeType.CHERRY),
        Map.entry("cherry_bees_005", org.bukkit.TreeType.CHERRY), Map.entry("pale_oak", org.bukkit.TreeType.PALE_OAK),
        Map.entry("pale_oak_bonemeal", org.bukkit.TreeType.PALE_OAK), Map.entry("pale_oak_creaking", org.bukkit.TreeType.PALE_OAK_CREAKING));

    /** Snapshots of the {@code [{x, y, z, state}]} a change is to make, for plugins to edit. */
    private List<org.bukkit.block.BlockState> blockStates(final World world, final Object list) {
        final List<org.bukkit.block.BlockState> out = new ArrayList<>();
        for (final Object b : Values.list(list)) {
            final Map<String, Object> block = Values.map(b);
            out.add(HostBlockState.of((HostBlock) world.getBlockAt(Values.i32(block, "x", 0), Values.i32(block, "y", 0), Values.i32(block, "z", 0)), Values.str(block, "state")));
        }
        return out;
    }

    private static List<Object> blockStatesValue(final List<org.bukkit.block.BlockState> states) {
        final List<Object> out = new ArrayList<>();
        for (final org.bukkit.block.BlockState state : states) {
            out.add(Values.of("x", (long) state.getX(), "y", (long) state.getY(), "z", (long) state.getZ(), "state", state.getBlockData().getAsString()));
        }
        return out;
    }

    /** {@code PlayerBedEnterEvent#enterAction}: what the bed's rules allow there, and what stood in the way. */
    private record HostBedEnterAction(io.papermc.paper.block.bed.BedRuleResult canSleep, io.papermc.paper.block.bed.BedRuleResult canSetSpawn,
                                      io.papermc.paper.block.bed.@Nullable BedEnterProblem problem, net.kyori.adventure.text.@Nullable Component errorMessage)
        implements io.papermc.paper.block.bed.BedEnterAction {
    }

    private static io.papermc.paper.block.bed.BedRuleResult bedRule(final String name) {
        return switch (name) {
            case "NEVER" -> io.papermc.paper.block.bed.BedRuleResult.NEVER;
            case "TOO_MUCH_LIGHT" -> io.papermc.paper.block.bed.BedRuleResult.TOO_MUCH_LIGHT;
            default -> io.papermc.paper.block.bed.BedRuleResult.ALLOWED;
        };
    }

    private static io.papermc.paper.block.bed.@Nullable BedEnterProblem bedProblem(final org.bukkit.event.player.PlayerBedEnterEvent.BedEnterResult result) {
        return switch (result) {
            case TOO_FAR_AWAY -> io.papermc.paper.block.bed.BedEnterProblem.TOO_FAR_AWAY;
            case OBSTRUCTED -> io.papermc.paper.block.bed.BedEnterProblem.OBSTRUCTED;
            case NOT_SAFE -> io.papermc.paper.block.bed.BedEnterProblem.NOT_SAFE;
            case EXPLOSION -> io.papermc.paper.block.bed.BedEnterProblem.EXPLOSION;
            case OTHER_PROBLEM -> io.papermc.paper.block.bed.BedEnterProblem.OTHER;
            default -> null;
        };
    }

    private static org.bukkit.entity.EntityType entityType(final String key) {
        final org.bukkit.NamespacedKey name = org.bukkit.NamespacedKey.fromString(key);
        for (final org.bukkit.entity.EntityType type : org.bukkit.entity.EntityType.values()) {
            if (type != org.bukkit.entity.EntityType.UNKNOWN && type.getKey().equals(name)) {
                return type;
            }
        }
        return org.bukkit.entity.EntityType.CHICKEN;
    }

    /** The living entities a list of entity references names. */
    private List<org.bukkit.entity.LivingEntity> living(final Object list) {
        final List<org.bukkit.entity.LivingEntity> out = new ArrayList<>();
        for (final Object ref : Values.list(list)) {
            if (ref instanceof Map<?, ?> m && io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(m)) instanceof org.bukkit.entity.LivingEntity living) {
                out.add(living);
            }
        }
        return out;
    }

    private static List<Object> entityIds(final java.util.Collection<? extends org.bukkit.entity.Entity> entities) {
        final List<Object> out = new ArrayList<>();
        for (final org.bukkit.entity.Entity entity : entities) {
            out.add((long) entity.getEntityId());
        }
        return out;
    }

    /** What a projectile hit: {@code hit_entity}, or {@code hit_block} on side {@code hit_face}. */
    private @Nullable org.bukkit.entity.Entity hitEntity(final Map<String, Object> args) {
        return args.get("hit_entity") instanceof Map<?, ?> ref ? io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(ref)) : null;
    }

    private @Nullable Block hitBlock(final Map<String, Object> args) {
        return args.get("hit_block") == null ? null : this.at(this.server.getWorld(Values.str(args, "world")), args.get("hit_block"));
    }

    private @Nullable org.bukkit.block.BlockFace hitFace(final Map<String, Object> args) {
        return args.get("hit_face") instanceof String face ? org.bukkit.block.BlockFace.valueOf(face) : null;
    }

    /** The block at an {@code [x, y, z]} list. */
    private Block at(final World world, final Object xyz) {
        final List<Object> at = Values.list(xyz);
        return world.getBlockAt(((Number) at.get(0)).intValue(), ((Number) at.get(1)).intValue(), ((Number) at.get(2)).intValue());
    }

    /**
     * Paper's {@code SignBlock#openTextEdit}: {@code PlayerOpenSignEvent}, then Bukkit's
     * {@code PlayerSignOpenEvent} if anyone listens; either may keep the editor shut.
     */
    private Map<String, Object> signOpen(final Map<String, Object> args) {
        final HostPlayer player = this.player(args);
        if (!(this.block(args).getState() instanceof org.bukkit.block.Sign sign)) {
            return Values.of();
        }
        final org.bukkit.block.sign.Side side = Values.bool(args, "front", true) ? org.bukkit.block.sign.Side.FRONT : org.bukkit.block.sign.Side.BACK;
        final String cause = Values.str(args, "cause", "UNKNOWN");
        if (!new io.papermc.paper.event.player.PlayerOpenSignEvent(player, sign, side, io.papermc.paper.event.player.PlayerOpenSignEvent.Cause.valueOf(cause)).callEvent()) {
            return Values.of("cancelled", true);
        }
        if (org.bukkit.event.player.PlayerSignOpenEvent.getHandlerList().getRegisteredListeners().length > 0
            && !new org.bukkit.event.player.PlayerSignOpenEvent(player, sign, side, org.bukkit.event.player.PlayerSignOpenEvent.Cause.valueOf(cause)).callEvent()) {
            return Values.of("cancelled", true);
        }
        return Values.of("cancelled", false);
    }

    private Block block(final Map<String, Object> args) {
        final World world = this.server.getWorld(Values.str(args, "world"));
        return world.getBlockAt(Values.i32(args, "x", 0), Values.i32(args, "y", 0), Values.i32(args, "z", 0));
    }

    public Object handle(final String method, final Map<String, Object> args) {
        return switch (method) {
            case "event.join" -> this.join(args);
            case "event.quit" -> this.quit(args);
            case "event.command_preprocess" -> {
                final PlayerCommandPreprocessEvent event = new PlayerCommandPreprocessEvent(this.player(args), Values.str(args, "message"));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "message", event.getMessage());
            }
            case "event.server_command" -> {
                final ServerCommandEvent event = new ServerCommandEvent(this.server.getConsoleSender(), Values.str(args, "command"));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "command", event.getCommand());
            }
            case "event.resource_pack_status" -> {
                final org.bukkit.event.player.PlayerResourcePackStatusEvent.Status[] all = org.bukkit.event.player.PlayerResourcePackStatusEvent.Status.values();
                final int status = Values.i32(args, "status", 0);
                if (status >= 0 && status < all.length) {
                    new org.bukkit.event.player.PlayerResourcePackStatusEvent(this.player(args), Values.uuid(args, "id"), all[status]).callEvent();
                }
                yield Values.of();
            }
            case "event.sign_change" -> {
                final List<net.kyori.adventure.text.Component> lines = new ArrayList<>();
                for (final Object line : Values.list(args.get("lines"))) {
                    final net.kyori.adventure.text.Component c = io.papermc.paperrs.host.Components.fromJson(line);
                    lines.add(c == null ? net.kyori.adventure.text.Component.empty() : c);
                }
                final org.bukkit.event.block.SignChangeEvent event = new org.bukkit.event.block.SignChangeEvent(this.block(args), this.player(args), lines,
                    Values.bool(args, "front", true) ? org.bukkit.block.sign.Side.FRONT : org.bukkit.block.sign.Side.BACK);
                event.callEvent();
                final List<Object> out = new ArrayList<>();
                for (final net.kyori.adventure.text.Component line : event.lines()) {
                    out.add(io.papermc.paperrs.host.Components.toJson(line == null ? net.kyori.adventure.text.Component.empty() : line));
                }
                yield Values.of("cancelled", event.isCancelled(), "lines", out);
            }
            case "event.interact" -> this.interact(args);
            case "event.interact_entity" -> {
                // Paper fires PlayerInteractAtEntityEvent for every right-click on an entity;
                // PlayerInteractEntityEvent listeners hear it too.
                final Object target = args.get("entity");
                final org.bukkit.entity.Entity entity = target instanceof Map<?, ?> ref ? io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(ref)) : null;
                if (entity == null) {
                    yield Values.of();
                }
                final List<Object> at = Values.list(args.get("at"));
                final org.bukkit.util.Vector position = at.size() == 3
                    ? new org.bukkit.util.Vector(((Number) at.get(0)).doubleValue(), ((Number) at.get(1)).doubleValue(), ((Number) at.get(2)).doubleValue())
                    : new org.bukkit.util.Vector();
                final org.bukkit.event.player.PlayerInteractAtEntityEvent event = new org.bukkit.event.player.PlayerInteractAtEntityEvent(this.player(args), entity, position,
                    Values.i32(args, "hand", 0) == 1 ? EquipmentSlot.OFF_HAND : EquipmentSlot.HAND);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.entity_explode", "event.block_explode" -> this.explode(args);
            case "event.toggle_sneak" -> {
                final org.bukkit.event.player.PlayerToggleSneakEvent event = new org.bukkit.event.player.PlayerToggleSneakEvent(this.player(args), Values.bool(args, "sneaking", false));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.toggle_sprint" -> {
                final org.bukkit.event.player.PlayerToggleSprintEvent event = new org.bukkit.event.player.PlayerToggleSprintEvent(this.player(args), Values.bool(args, "sprinting", false));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.toggle_flight" -> {
                final org.bukkit.event.player.PlayerToggleFlightEvent event = new org.bukkit.event.player.PlayerToggleFlightEvent(this.player(args), Values.bool(args, "flying", false));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.item_held" -> {
                final org.bukkit.event.player.PlayerItemHeldEvent event = new org.bukkit.event.player.PlayerItemHeldEvent(this.player(args), Values.i32(args, "previous", 0), Values.i32(args, "new", 0));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.swap_hands" -> {
                final org.bukkit.event.player.PlayerSwapHandItemsEvent event = new org.bukkit.event.player.PlayerSwapHandItemsEvent(this.player(args),
                    io.papermc.paperrs.host.item.HostItemStack.fromValue(args.get("main")), io.papermc.paperrs.host.item.HostItemStack.fromValue(args.get("off")));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "main", io.papermc.paperrs.host.item.HostItemStack.toValue(event.getMainHandItem()),
                    "off", io.papermc.paperrs.host.item.HostItemStack.toValue(event.getOffHandItem()));
            }
            case "event.item_consume" -> {
                final org.bukkit.event.player.PlayerItemConsumeEvent event = new org.bukkit.event.player.PlayerItemConsumeEvent(this.player(args),
                    io.papermc.paperrs.host.item.HostItemStack.fromValue(args.get("item")), Values.i32(args, "hand", 0) == 1 ? EquipmentSlot.OFF_HAND : EquipmentSlot.HAND);
                event.callEvent();
                final org.bukkit.inventory.ItemStack replacement = event.getReplacement();
                yield Values.of("cancelled", event.isCancelled(), "item", io.papermc.paperrs.host.item.HostItemStack.toValue(event.getItem()),
                    "replacement", replacement == null ? null : io.papermc.paperrs.host.item.HostItemStack.toValue(replacement));
            }
            case "event.food_level_change" -> {
                final Object item = args.get("item");
                final org.bukkit.event.entity.FoodLevelChangeEvent event = new org.bukkit.event.entity.FoodLevelChangeEvent(this.player(args), Values.i32(args, "level", 0),
                    item == null ? null : io.papermc.paperrs.host.item.HostItemStack.fromValue(item));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "level", event.getFoodLevel());
            }
            case "event.bucket_empty", "event.bucket_fill" -> this.bucket(args, method.equals("event.bucket_fill"));
            case "event.weather_change" -> {
                final World world = this.server.getWorld(Values.str(args, "world"));
                final boolean to = Values.bool(args, "to", false);
                final String cause = Values.str(args, "cause", "UNKNOWN");
                final boolean cancelled;
                if (Values.bool(args, "thunder", false)) {
                    final org.bukkit.event.weather.ThunderChangeEvent event = new org.bukkit.event.weather.ThunderChangeEvent(world, to, org.bukkit.event.weather.ThunderChangeEvent.Cause.valueOf(cause));
                    event.callEvent();
                    cancelled = event.isCancelled();
                } else {
                    final org.bukkit.event.weather.WeatherChangeEvent event = new org.bukkit.event.weather.WeatherChangeEvent(world, to, org.bukkit.event.weather.WeatherChangeEvent.Cause.valueOf(cause));
                    event.callEvent();
                    cancelled = event.isCancelled();
                }
                yield Values.of("cancelled", cancelled);
            }
            case "event.teleport", "event.portal" -> {
                final HostPlayer player = this.player(args);
                final Location from = this.worldLocation(Values.map(args.get("from")));
                final Location to = this.worldLocation(Values.map(args.get("to")));
                final org.bukkit.event.player.PlayerTeleportEvent.TeleportCause cause = org.bukkit.event.player.PlayerTeleportEvent.TeleportCause.valueOf(Values.str(args, "cause", "UNKNOWN"));
                final org.bukkit.event.player.PlayerTeleportEvent event = method.equals("event.portal")
                    ? new org.bukkit.event.player.PlayerPortalEvent(player, from, to, cause, Values.i32(args, "search_radius", 128), Values.bool(args, "can_create", true), Values.i32(args, "creation_radius", 16))
                    : new org.bukkit.event.player.PlayerTeleportEvent(player, from, to, cause);
                event.callEvent();
                final Location target = event.getTo();
                if (event.isCancelled() || target == null) {
                    yield Values.of("cancelled", true);
                }
                final World world = target.getWorld() != null ? target.getWorld() : to.getWorld();
                yield Values.of("to", Values.of("world", world.getName(), "x", target.getX(), "y", target.getY(), "z", target.getZ(),
                    "yaw", (double) target.getYaw(), "pitch", (double) target.getPitch()));
            }
            case "event.entity_target" -> {
                final org.bukkit.entity.Entity entity = io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity")));
                final org.bukkit.entity.Entity target = io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("target")));
                if (entity == null || !(target instanceof org.bukkit.entity.LivingEntity living)) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.EntityTargetLivingEntityEvent event = new org.bukkit.event.entity.EntityTargetLivingEntityEvent(entity, living,
                    org.bukkit.event.entity.EntityTargetEvent.TargetReason.valueOf(Values.str(args, "reason", "UNKNOWN")));
                event.callEvent();
                // A listener that changes the target to nothing keeps the mob from targeting.
                yield Values.of("cancelled", event.isCancelled() || event.getTarget() != living);
            }
            case "event.block_changes" -> this.blockChanges(args);
            case "event.structure_grow" -> {
                final World world = this.server.getWorld(Values.str(args, "world"));
                final Location at = new Location(world, Values.i32(args, "x", 0), Values.i32(args, "y", 0), Values.i32(args, "z", 0));
                final List<org.bukkit.block.BlockState> blocks = this.blockStates(world, args.get("blocks"));
                final HostPlayer player = args.get("uuid") == null ? null : this.player(args);
                final org.bukkit.event.world.StructureGrowEvent event = new org.bukkit.event.world.StructureGrowEvent(at,
                    TREE_TYPES.getOrDefault(Values.str(args, "feature"), org.bukkit.TreeType.TREE), Values.bool(args, "bonemeal", false), player, blocks);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "blocks", blockStatesValue(event.getBlocks()));
            }
            case "event.fertilize" -> {
                final World world = this.server.getWorld(Values.str(args, "world"));
                final List<org.bukkit.block.BlockState> blocks = this.blockStates(world, args.get("blocks"));
                final HostPlayer player = args.get("uuid") == null ? null : this.player(args);
                final org.bukkit.event.block.BlockFertilizeEvent event = new org.bukkit.event.block.BlockFertilizeEvent(this.block(args), player, blocks);
                event.setCancelled(Values.bool(args, "cancelled", false));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "blocks", blockStatesValue(event.getBlocks()));
            }
            case "event.portal_create" -> {
                final World world = this.server.getWorld(Values.str(args, "world"));
                final org.bukkit.entity.Entity entity = args.get("entity") instanceof Map<?, ?> ref ? io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(ref)) : null;
                final org.bukkit.event.world.PortalCreateEvent event = new org.bukkit.event.world.PortalCreateEvent(this.blockStates(world, args.get("blocks")), world, entity,
                    org.bukkit.event.world.PortalCreateEvent.CreateReason.valueOf(Values.str(args, "reason", "FIRE")));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.water_splash" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.ThrownPotion potion)) {
                    yield Values.of();
                }
                final Map<org.bukkit.entity.LivingEntity, Double> damage = new java.util.HashMap<>();
                for (final org.bukkit.entity.LivingEntity living : this.living(args.get("damage"))) {
                    damage.put(living, 1.0);
                }
                final io.papermc.paper.event.entity.WaterBottleSplashEvent event = new io.papermc.paper.event.entity.WaterBottleSplashEvent(potion,
                    this.hitEntity(args), this.hitBlock(args), this.hitFace(args), damage,
                    new java.util.HashSet<>(this.living(args.get("rehydrate"))), new java.util.HashSet<>(this.living(args.get("extinguish"))));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "damage", entityIds(event.getToDamage()),
                    "extinguish", entityIds(event.getToExtinguish()), "rehydrate", entityIds(event.getToRehydrate()));
            }
            case "event.potion_splash" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.ThrownPotion potion)) {
                    yield Values.of();
                }
                final Map<org.bukkit.entity.LivingEntity, Double> affected = new java.util.HashMap<>();
                for (final Object a : Values.list(args.get("affected"))) {
                    final Map<String, Object> entry = Values.map(a);
                    if (io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(entry.get("entity"))) instanceof org.bukkit.entity.LivingEntity living) {
                        affected.put(living, Values.f64(entry, "intensity", 0));
                    }
                }
                final org.bukkit.event.entity.PotionSplashEvent event = new org.bukkit.event.entity.PotionSplashEvent(potion, this.hitEntity(args), this.hitBlock(args), this.hitFace(args), affected);
                event.callEvent();
                final List<Object> out = new ArrayList<>();
                for (final org.bukkit.entity.LivingEntity living : event.getAffectedEntities()) {
                    out.add(Values.of("id", (long) living.getEntityId(), "intensity", event.getIntensity(living)));
                }
                yield Values.of("cancelled", event.isCancelled(), "affected", out);
            }
            case "event.lingering_splash" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.ThrownPotion potion)
                    || !(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("cloud"))) instanceof org.bukkit.entity.AreaEffectCloud cloud)) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.LingeringPotionSplashEvent event = new org.bukkit.event.entity.LingeringPotionSplashEvent(potion,
                    this.hitEntity(args), this.hitBlock(args), this.hitFace(args), cloud);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.cloud_apply" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.AreaEffectCloud cloud)) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.AreaEffectCloudApplyEvent event = new org.bukkit.event.entity.AreaEffectCloudApplyEvent(cloud, this.living(args.get("affected")));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "affected", entityIds(event.getAffectedEntities()));
            }
            case "event.item_damage" -> {
                final org.bukkit.event.player.PlayerItemDamageEvent event = new org.bukkit.event.player.PlayerItemDamageEvent(this.player(args),
                    io.papermc.paperrs.host.item.HostItemStack.fromValue(args.get("item")), Values.i32(args, "damage", 0), Values.i32(args, "original", 0));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "damage", (long) event.getDamage());
            }
            case "event.item_break" -> {
                new org.bukkit.event.player.PlayerItemBreakEvent(this.player(args), io.papermc.paperrs.host.item.HostItemStack.fromValue(args.get("item"))).callEvent();
                yield Values.of();
            }
            case "event.shear" -> {
                final org.bukkit.entity.Entity entity = io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity")));
                if (entity == null) {
                    yield Values.of();
                }
                final org.bukkit.inventory.ItemStack tool = io.papermc.paperrs.host.item.HostItemStack.fromValue(args.get("item"));
                final List<org.bukkit.inventory.ItemStack> drops = new ArrayList<>();
                for (final Object drop : Values.list(args.get("drops"))) {
                    drops.add(io.papermc.paperrs.host.item.HostItemStack.fromValue(drop));
                }
                final List<org.bukkit.inventory.ItemStack> left;
                final boolean cancelled;
                if (args.get("uuid") != null) {
                    final org.bukkit.event.player.PlayerShearEntityEvent event = new org.bukkit.event.player.PlayerShearEntityEvent(this.player(args), entity, tool,
                        Values.i32(args, "hand", 0) == 1 ? EquipmentSlot.OFF_HAND : EquipmentSlot.HAND, drops);
                    event.callEvent();
                    left = event.getDrops();
                    cancelled = event.isCancelled();
                } else {
                    final org.bukkit.event.block.BlockShearEntityEvent event = new org.bukkit.event.block.BlockShearEntityEvent(
                        this.at(this.server.getWorld(Values.str(args, "world")), args.get("block")), entity, tool, drops);
                    event.callEvent();
                    left = event.getDrops();
                    cancelled = event.isCancelled();
                }
                final List<Object> out = new ArrayList<>();
                for (final org.bukkit.inventory.ItemStack drop : left) {
                    out.add(io.papermc.paperrs.host.item.HostItemStack.toValue(drop));
                }
                yield Values.of("cancelled", cancelled, "drops", out);
            }
            case "event.bed_enter" -> {
                final net.kyori.adventure.text.Component message = args.get("message") == null ? null : io.papermc.paperrs.host.Components.fromJson(args.get("message"));
                final org.bukkit.event.player.PlayerBedEnterEvent.BedEnterResult result = org.bukkit.event.player.PlayerBedEnterEvent.BedEnterResult.valueOf(Values.str(args, "result", "OK"));
                final org.bukkit.event.player.PlayerBedEnterEvent event = new org.bukkit.event.player.PlayerBedEnterEvent(this.player(args), this.block(args), result,
                    new HostBedEnterAction(bedRule(Values.str(args, "can_sleep", "ALLOWED")), bedRule(Values.str(args, "can_set_spawn", "ALLOWED")), bedProblem(result), message));
                event.callEvent();
                yield Values.of("use", event.useBed().name());
            }
            case "event.bed_leave" -> {
                final org.bukkit.event.player.PlayerBedLeaveEvent event = new org.bukkit.event.player.PlayerBedLeaveEvent(this.player(args), this.block(args), true);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.main_hand" -> {
                new org.bukkit.event.player.PlayerChangedMainHandEvent(this.player(args), org.bukkit.inventory.MainHand.valueOf(Values.str(args, "hand", "RIGHT"))).callEvent();
                yield Values.of();
            }
            case "event.locale" -> {
                new org.bukkit.event.player.PlayerLocaleChangeEvent(this.player(args), Values.str(args, "locale", "en_us")).callEvent();
                yield Values.of();
            }
            case "event.pickup_arrow" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("arrow"))) instanceof org.bukkit.entity.AbstractArrow arrow)
                    || !(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("item"))) instanceof org.bukkit.entity.Item item)) {
                    yield Values.of();
                }
                final org.bukkit.event.player.PlayerPickupArrowEvent event = new org.bukkit.event.player.PlayerPickupArrowEvent(this.player(args), item, arrow);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.egg" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.Egg egg)) {
                    yield Values.of();
                }
                boolean hatching = Values.bool(args, "hatching", false);
                byte count = (byte) Values.i32(args, "count", 0);
                org.bukkit.entity.EntityType type = entityType(Values.str(args, "type", "minecraft:chicken"));
                if (Values.bool(args, "throw", false) && args.get("uuid") != null) {
                    final org.bukkit.event.player.PlayerEggThrowEvent event = new org.bukkit.event.player.PlayerEggThrowEvent(this.player(args), egg, hatching, count, type);
                    event.callEvent();
                    hatching = event.isHatching();
                    count = hatching ? event.getNumHatches() : 0;
                    type = event.getHatchingType();
                }
                if (Values.bool(args, "hatch", false)) {
                    final com.destroystokyo.paper.event.entity.ThrownEggHatchEvent event = new com.destroystokyo.paper.event.entity.ThrownEggHatchEvent(egg, hatching, count, type);
                    event.callEvent();
                    hatching = event.isHatching();
                    count = hatching ? event.getNumHatches() : 0;
                    type = event.getHatchingType();
                }
                yield Values.of("hatching", hatching, "count", (long) count, "type", type.getKey().toString());
            }
            case "event.exp_bottle" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.ThrownExpBottle bottle)) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.ExpBottleEvent event = new org.bukkit.event.entity.ExpBottleEvent(bottle, this.hitEntity(args), this.hitBlock(args), this.hitFace(args),
                    Values.i32(args, "experience", 0));
                event.callEvent();
                yield Values.of("experience", (long) event.getExperience(), "show", event.getShowEffect());
            }
            case "event.potion_effect" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.LivingEntity living)) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.EntityPotionEffectEvent event = new org.bukkit.event.entity.EntityPotionEffectEvent(living,
                    io.papermc.paperrs.host.entity.Effects.of(args.get("old")), io.papermc.paperrs.host.entity.Effects.of(args.get("new")), null,
                    org.bukkit.event.entity.EntityPotionEffectEvent.Cause.valueOf(Values.str(args, "cause", "UNKNOWN")),
                    org.bukkit.event.entity.EntityPotionEffectEvent.Action.valueOf(Values.str(args, "action", "ADDED")), Values.bool(args, "override", false));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "override", event.isOverride());
            }
            case "event.shoot_bow" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.LivingEntity shooter)) {
                    yield Values.of();
                }
                final org.bukkit.inventory.ItemStack consumable = io.papermc.paperrs.host.item.HostItemStack.fromValue(args.get("consumable"));
                final org.bukkit.event.entity.EntityShootBowEvent event = new org.bukkit.event.entity.EntityShootBowEvent(shooter,
                    io.papermc.paperrs.host.item.HostItemStack.fromValue(args.get("bow")), consumable == null || consumable.isEmpty() ? null : consumable,
                    io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("projectile"))),
                    Values.i32(args, "hand", 0) == 1 ? EquipmentSlot.OFF_HAND : EquipmentSlot.HAND, (float) Values.f64(args, "force", 1), Values.bool(args, "consume", true));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "consume", event.shouldConsumeItem());
            }
            case "event.item_spawn" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.Item item)) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.ItemSpawnEvent event = new org.bukkit.event.entity.ItemSpawnEvent(item);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.spawner_spawn" -> {
                final org.bukkit.entity.Entity entity = io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity")));
                if (entity == null || !(this.block(args).getState() instanceof org.bukkit.block.CreatureSpawner spawner)) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.SpawnerSpawnEvent event = new org.bukkit.event.entity.SpawnerSpawnEvent(entity, spawner);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.sign_open" -> this.signOpen(args);
            case "event.unknown_command" -> {
                final org.bukkit.event.command.UnknownCommandEvent event = new org.bukkit.event.command.UnknownCommandEvent(
                    io.papermc.paperrs.host.command.HostCommandSourceStack.of(this.server.senderFor(Values.str(args, "sender", "console"))),
                    Values.str(args, "command", ""), Components.fromJson(args.get("message")));
                event.callEvent();
                yield Values.of("message", event.message() == null ? null : Components.toJson(event.message()));
            }
            case "event.client_loaded" -> {
                new io.papermc.paper.event.player.PlayerClientLoadedWorldEvent(this.player(args), Values.bool(args, "timeout", false)).callEvent();
                yield Values.of();
            }
            case "event.pre_dispense" -> {
                final io.papermc.paper.event.block.BlockPreDispenseEvent event = new io.papermc.paper.event.block.BlockPreDispenseEvent(this.block(args),
                    io.papermc.paperrs.host.item.HostItemStack.fromValue(args.get("item")), Values.i32(args, "slot", 0));
                yield Values.of("cancelled", !event.callEvent());
            }
            case "event.failed_dispense" -> Values.of("effect", new io.papermc.paper.event.block.BlockFailedDispenseEvent(this.block(args)).callEvent());
            case "event.item_despawn" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.Item item)) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.ItemDespawnEvent event = new org.bukkit.event.entity.ItemDespawnEvent(item, item.getLocation());
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.item_merge" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.Item item)
                    || !(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("target"))) instanceof org.bukkit.entity.Item target)) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.ItemMergeEvent event = new org.bukkit.event.entity.ItemMergeEvent(item, target);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.tnt_prime" -> {
                final org.bukkit.entity.Entity entity = args.get("entity") instanceof Map<?, ?> ref ? io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(ref)) : null;
                final Block from = args.get("block") == null ? null : this.at(this.server.getWorld(Values.str(args, "world")), args.get("block"));
                final org.bukkit.event.block.TNTPrimeEvent event = new org.bukkit.event.block.TNTPrimeEvent(this.block(args),
                    org.bukkit.event.block.TNTPrimeEvent.PrimeCause.valueOf(Values.str(args, "cause", "EXPLOSION")), entity, from);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.explosion_prime" -> {
                final org.bukkit.entity.Entity entity = io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity")));
                if (entity == null) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.ExplosionPrimeEvent event = new org.bukkit.event.entity.ExplosionPrimeEvent(entity, (float) Values.f64(args, "radius", 0), Values.bool(args, "fire", false));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "radius", (double) event.getRadius(), "fire", event.getFire());
            }
            case "event.lightning" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.LightningStrike bolt)) {
                    yield Values.of();
                }
                final org.bukkit.event.weather.LightningStrikeEvent event = new org.bukkit.event.weather.LightningStrikeEvent(this.server.getWorld(Values.str(args, "world")), bolt,
                    org.bukkit.event.weather.LightningStrikeEvent.Cause.valueOf(Values.str(args, "cause", "UNKNOWN")));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.creeper_power" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.Creeper creeper)) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.CreeperPowerEvent.PowerCause cause = org.bukkit.event.entity.CreeperPowerEvent.PowerCause.valueOf(Values.str(args, "cause", "LIGHTNING"));
                final org.bukkit.event.entity.CreeperPowerEvent event = args.get("bolt") instanceof Map<?, ?> ref
                    && io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(ref)) instanceof org.bukkit.entity.LightningStrike bolt
                    ? new org.bukkit.event.entity.CreeperPowerEvent(creeper, bolt, cause) : new org.bukkit.event.entity.CreeperPowerEvent(creeper, cause);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.zap" -> {
                final org.bukkit.entity.Entity entity = io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity")));
                final org.bukkit.entity.Entity into = io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("into")));
                if (entity == null || into == null || !(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("bolt"))) instanceof org.bukkit.entity.LightningStrike bolt)) {
                    yield Values.of();
                }
                if (Values.bool(args, "pig", false) && entity instanceof org.bukkit.entity.Pig pig && into instanceof org.bukkit.entity.PigZombie piglin) {
                    final org.bukkit.event.entity.PigZapEvent event = new org.bukkit.event.entity.PigZapEvent(pig, bolt, piglin);
                    event.callEvent();
                    yield Values.of("cancelled", event.isCancelled());
                }
                final com.destroystokyo.paper.event.entity.EntityZapEvent zap = new com.destroystokyo.paper.event.entity.EntityZapEvent(entity, bolt, into);
                if (!zap.callEvent()) {
                    yield Values.of("cancelled", true);
                }
                final org.bukkit.event.entity.EntityTransformEvent event = new org.bukkit.event.entity.EntityTransformEvent(entity, List.of(into),
                    org.bukkit.event.entity.EntityTransformEvent.TransformReason.LIGHTNING);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.vehicle_damage" -> this.entities.vehicleDamage(args, false);
            case "event.vehicle_destroy" -> this.entities.vehicleDamage(args, true);
            case "event.vehicle_create" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.Vehicle vehicle)) {
                    yield Values.of();
                }
                final org.bukkit.event.vehicle.VehicleCreateEvent event = new org.bukkit.event.vehicle.VehicleCreateEvent(vehicle);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.entity_place" -> {
                final org.bukkit.entity.Entity entity = io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity")));
                if (entity == null) {
                    yield Values.of();
                }
                final org.bukkit.event.entity.EntityPlaceEvent event = new org.bukkit.event.entity.EntityPlaceEvent(entity, this.player(args), this.block(args),
                    org.bukkit.block.BlockFace.valueOf(Values.str(args, "face", "UP")), Values.i32(args, "hand", 0) == 1 ? EquipmentSlot.OFF_HAND : EquipmentSlot.HAND);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.vehicle_move" -> {
                if (!(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))) instanceof org.bukkit.entity.Vehicle vehicle)) {
                    yield Values.of();
                }
                final List<Object> from = Values.list(args.get("from"));
                final Location before = new Location(vehicle.getWorld(), ((Number) from.get(0)).doubleValue(), ((Number) from.get(1)).doubleValue(), ((Number) from.get(2)).doubleValue(),
                    ((Number) from.get(3)).floatValue(), ((Number) from.get(4)).floatValue());
                new org.bukkit.event.vehicle.VehicleMoveEvent(vehicle, before, vehicle.getLocation()).callEvent();
                yield Values.of();
            }
            case "event.projectile_launch" -> {
                final org.bukkit.event.entity.ProjectileLaunchEvent event = new org.bukkit.event.entity.ProjectileLaunchEvent(
                    io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.projectile_hit" -> {
                final org.bukkit.entity.Entity projectile = io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity")));
                final org.bukkit.entity.Entity hit = args.get("hit_entity") instanceof Map<?, ?> ref ? io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(ref)) : null;
                final Block block = args.get("hit_block") == null ? null : this.at(this.server.getWorld(Values.str(args, "world")), args.get("hit_block"));
                if (!(projectile instanceof org.bukkit.entity.Projectile thrown)) {
                    yield Values.of("cancelled", false);
                }
                final org.bukkit.event.entity.ProjectileHitEvent event = new org.bukkit.event.entity.ProjectileHitEvent(thrown, hit, block);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.dispense" -> {
                final List<Object> velocity = Values.list(args.get("velocity"));
                final org.bukkit.event.block.BlockDispenseEvent event = new org.bukkit.event.block.BlockDispenseEvent(this.block(args),
                    io.papermc.paperrs.host.item.HostItemStack.fromValue(args.get("item")),
                    new org.bukkit.util.Vector(((Number) velocity.get(0)).doubleValue(), ((Number) velocity.get(1)).doubleValue(), ((Number) velocity.get(2)).doubleValue()));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "item", io.papermc.paperrs.host.item.HostItemStack.toValue(event.getItem()));
            }
            case "event.block_damage" -> {
                final HostPlayer player = this.player(args);
                final org.bukkit.event.block.BlockDamageEvent event = new org.bukkit.event.block.BlockDamageEvent(player, this.block(args),
                    org.bukkit.block.BlockFace.valueOf(Values.str(args, "face", "UP")), player.getInventory().getItemInMainHand(), Values.bool(args, "insta", false));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "insta", event.getInstaBreak());
            }
            case "event.kick" -> {
                final org.bukkit.event.player.PlayerKickEvent event = new org.bukkit.event.player.PlayerKickEvent(this.player(args), Components.fromJson(args.get("reason")),
                    Components.fromJson(args.get("leave")), org.bukkit.event.player.PlayerKickEvent.Cause.valueOf(Values.str(args, "cause", "UNKNOWN")));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "reason", Components.toJson(event.reason()), "leave", Components.toJson(event.leaveMessage()));
            }
            case "event.regain_health" -> {
                final org.bukkit.event.entity.EntityRegainHealthEvent event = new org.bukkit.event.entity.EntityRegainHealthEvent(
                    io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))), Values.f64(args, "amount", 0),
                    org.bukkit.event.entity.EntityRegainHealthEvent.RegainReason.valueOf(Values.str(args, "reason", "CUSTOM")));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "amount", event.getAmount());
            }
            case "event.combust" -> {
                final org.bukkit.entity.Entity entity = io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity")));
                final float seconds = (float) Values.f64(args, "seconds", 0);
                final org.bukkit.event.entity.EntityCombustEvent event;
                if (args.get("by_entity") instanceof Map<?, ?> by) {
                    event = new org.bukkit.event.entity.EntityCombustByEntityEvent(io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(by)), entity, seconds);
                } else if (args.get("by_block") != null) {
                    event = new org.bukkit.event.entity.EntityCombustByBlockEvent(this.at(this.server.getWorld(Values.str(args, "world")), args.get("by_block")), entity, seconds);
                } else {
                    event = new org.bukkit.event.entity.EntityCombustEvent(entity, seconds);
                }
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled(), "seconds", (double) event.getDuration());
            }
            case "event.entity_change_block" -> {
                final org.bukkit.event.entity.EntityChangeBlockEvent event = new org.bukkit.event.entity.EntityChangeBlockEvent(
                    io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(args.get("entity"))), this.block(args), org.bukkit.Bukkit.createBlockData(Values.str(args, "to")));
                event.setCancelled(Values.bool(args, "cancelled", false));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.block_flows" -> {
                // Fluids flowing from a block towards a face, one BlockFromToEvent each.
                final World world = this.server.getWorld(Values.str(args, "world"));
                final List<Object> results = new ArrayList<>();
                for (final Object f : Values.list(args.get("flows"))) {
                    final Map<String, Object> flow = Values.map(f);
                    final Block from = world.getBlockAt(Values.i32(flow, "x", 0), Values.i32(flow, "y", 0), Values.i32(flow, "z", 0));
                    final boolean allowed = new org.bukkit.event.block.BlockFromToEvent(from, org.bukkit.block.BlockFace.valueOf(Values.str(flow, "face"))).callEvent();
                    results.add(Values.of("cancelled", !allowed));
                }
                yield Values.of("results", results);
            }
            case "event.changed_world" -> {
                new org.bukkit.event.player.PlayerChangedWorldEvent(this.player(args), this.server.getWorld(Values.str(args, "from"))).callEvent();
                yield Values.of();
            }
            case "event.login" -> this.login(args);
            case "event.chunk_load" -> {
                final World world = this.server.getWorld(Values.str(args, "world"));
                new org.bukkit.event.world.ChunkLoadEvent(world.getChunkAt(Values.i32(args, "x", 0), Values.i32(args, "z", 0), false), Values.bool(args, "new", false)).callEvent();
                yield Values.of();
            }
            case "event.chunk_unload" -> {
                final World world = this.server.getWorld(Values.str(args, "world"));
                new org.bukkit.event.world.ChunkUnloadEvent(world.getChunkAt(Values.i32(args, "x", 0), Values.i32(args, "z", 0), false), true).callEvent();
                yield Values.of();
            }
            case "event.piston" -> {
                final Block piston = this.block(args);
                final List<Block> blocks = new ArrayList<>();
                for (final Object b : Values.list(args.get("blocks"))) {
                    blocks.add(this.at(piston.getWorld(), b));
                }
                final org.bukkit.block.BlockFace direction = org.bukkit.block.BlockFace.valueOf(Values.str(args, "direction"));
                // Paper's list can be read, not changed.
                final List<Block> view = java.util.Collections.unmodifiableList(blocks);
                final org.bukkit.event.block.BlockPistonEvent event = Values.bool(args, "extending", false)
                    ? new org.bukkit.event.block.BlockPistonExtendEvent(piston, view, direction)
                    : new org.bukkit.event.block.BlockPistonRetractEvent(piston, view, direction);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.block_burn" -> {
                final Block block = this.block(args);
                final org.bukkit.event.block.BlockBurnEvent event = new org.bukkit.event.block.BlockBurnEvent(block, this.at(block.getWorld(), args.get("by")));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.fire_spread" -> {
                // FireBlock.tick: BlockIgniteEvent (SPREAD), then BlockSpreadEvent with the fire's state.
                final Block block = this.block(args);
                final Block from = this.at(block.getWorld(), args.get("from"));
                final org.bukkit.event.block.BlockIgniteEvent ignite = new org.bukkit.event.block.BlockIgniteEvent(block, org.bukkit.event.block.BlockIgniteEvent.IgniteCause.SPREAD, from);
                ignite.callEvent();
                if (ignite.isCancelled()) {
                    yield Values.of("cancelled", true);
                }
                final org.bukkit.block.BlockState state = HostBlockState.of((HostBlock) block, Values.str(args, "state"));
                final org.bukkit.event.block.BlockSpreadEvent spread = new org.bukkit.event.block.BlockSpreadEvent(block, from, state);
                spread.callEvent();
                yield Values.of("cancelled", spread.isCancelled(), "state", state.getBlockData().getAsString());
            }
            case "event.block_ignite" -> {
                // Set by an entity (a player, lightning, a fireball's shooter, an explosion) or a
                // block (lava, a dispenser).
                final Block block = this.block(args);
                final org.bukkit.entity.Entity entity = args.get("entity") instanceof Map<?, ?> ref ? io.papermc.paperrs.host.entity.Entities.of(this.server, Values.map(ref)) : null;
                final Block from = args.get("from") == null ? null : this.at(block.getWorld(), args.get("from"));
                final org.bukkit.event.block.BlockIgniteEvent event = new org.bukkit.event.block.BlockIgniteEvent(block,
                    org.bukkit.event.block.BlockIgniteEvent.IgniteCause.valueOf(Values.str(args, "cause", "FLINT_AND_STEEL")), entity, from);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.block_break" -> {
                final BlockBreakEvent event = new BlockBreakEvent(this.block(args), this.player(args));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.block_place" -> {
                // The server places the block once the event allows it; until then the placed
                // block shows the new state and the replaced state is a snapshot of the old one.
                final HostBlock placed = ((HostBlock) this.block(args)).pending(Values.str(args, "state"));
                final List<Object> against = Values.list(args.get("against"));
                final Block clicked = placed.getWorld().getBlockAt(((Number) against.get(0)).intValue(), ((Number) against.get(1)).intValue(), ((Number) against.get(2)).intValue());
                final HostPlayer player = this.player(args);
                final EquipmentSlot hand = Values.bool(args, "offhand", false) ? EquipmentSlot.OFF_HAND : EquipmentSlot.HAND;
                final BlockPlaceEvent event = new BlockPlaceEvent(placed, HostBlockState.of(placed, Values.str(args, "replaced", "minecraft:air")), clicked,
                    player.getInventory().getItem(hand), player, true, hand);
                try {
                    event.callEvent();
                } finally {
                    placed.settle();
                }
                yield Values.of("cancelled", event.isCancelled() || !event.canBuild());
            }
            case "event.move" -> {
                final HostPlayer player = this.player(args);
                final Location from = this.location(player.getWorld(), Values.map(args.get("from")));
                final Location to = this.location(player.getWorld(), Values.map(args.get("to")));
                final PlayerMoveEvent event = new PlayerMoveEvent(player, from, to.clone());
                event.callEvent();
                if (event.isCancelled()) {
                    yield Values.of("cancelled", true);
                }
                final Location target = event.getTo();
                yield target.equals(to) ? Values.of() : Values.of("to", Values.of("x", target.getX(), "y", target.getY(), "z", target.getZ(),
                    "yaw", (double) target.getYaw(), "pitch", (double) target.getPitch()));
            }
            case "event.game_mode_change" -> {
                final PlayerGameModeChangeEvent event = new PlayerGameModeChangeEvent(this.player(args),
                    GameMode.valueOf(Values.str(args, "mode").toUpperCase(Locale.ROOT)), PlayerGameModeChangeEvent.Cause.UNKNOWN, null);
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.chat" -> this.chat(args);
            case "event.inventory_open" -> this.inventories.open(args);
            case "event.inventory_click" -> this.inventories.click(args);
            case "event.inventory_close" -> this.inventories.close(args);
            case "event.pickup_item" -> this.inventories.pickup(args);
            case "event.drop_item" -> this.inventories.drop(args);
            case "event.entity_damage" -> this.entities.damage(args);
            case "event.entity_death" -> this.entities.entityDeath(args);
            case "event.player_death" -> this.entities.playerDeath(args);
            case "event.respawn" -> this.entities.respawn(args);
            case "event.creature_spawn" -> this.entities.creatureSpawn(args);
            case "event.mount" -> this.entities.mount(args);
            case "event.dismount" -> this.entities.dismount(args);
            case "event.tame" -> this.entities.tame(args);
            case "event.statistic_increment" -> this.statisticIncrement(args);
            case "event.trade" -> this.merchants.trade(args);
            case "event.trade_select" -> this.merchants.select(args);
            case "event.villager_acquire_trade" -> this.merchants.acquire(args);
            case "event.villager_career_change" -> this.merchants.careerChange(args);
            case "event.villager_replenish_trade" -> this.merchants.replenish(args);
            case "event.server_list_ping" -> this.ping(args);
            case "event.pre_login" -> this.preLogin(args);
            default -> throw new IllegalArgumentException("unknown event " + method);
        };
    }

    /** A statistic goes up, unless the event is cancelled (CraftBukkit's handleStatisticsIncrease). */
    private Object statisticIncrement(final Map<String, Object> args) {
        final HostPlayer player = this.player(args);
        final String key = Values.str(args, "key");
        final org.bukkit.Statistic statistic = io.papermc.paperrs.host.entity.Statistics.statistic(Values.str(args, "type"), key);
        if (statistic == null) {
            return Values.of("cancelled", false);
        }
        final int previous = Values.i32(args, "old", 0);
        final int now = Values.i32(args, "new", 0);
        final org.bukkit.event.player.PlayerStatisticIncrementEvent event;
        if (statistic.getType() == org.bukkit.Statistic.Type.UNTYPED) {
            event = new org.bukkit.event.player.PlayerStatisticIncrementEvent(player, statistic, previous, now);
        } else if (statistic.getType() == org.bukkit.Statistic.Type.ENTITY) {
            final org.bukkit.entity.EntityType type = io.papermc.paperrs.host.entity.Statistics.entityType(key);
            if (type == null) {
                return Values.of("cancelled", false);
            }
            event = new org.bukkit.event.player.PlayerStatisticIncrementEvent(player, statistic, previous, now, type);
        } else {
            final org.bukkit.Material material = io.papermc.paperrs.host.entity.Statistics.material(key);
            if (material == null) {
                return Values.of("cancelled", false);
            }
            event = new org.bukkit.event.player.PlayerStatisticIncrementEvent(player, statistic, previous, now, material);
        }
        event.callEvent();
        return Values.of("cancelled", event.isCancelled());
    }

    /**
     * Block changes of the server's block engine, fired one after another ({@code
     * BlockGrowEvent}, {@code BlockFadeEvent}, {@code BlockFormEvent}, {@code BlockSpreadEvent},
     * {@code LeavesDecayEvent}, {@code MoistureChangeEvent}): whether each was cancelled, and
     * the state the block changes to as listeners leave it.
     */
    private Object blockChanges(final Map<String, Object> args) {
        final World world = this.server.getWorld(Values.str(args, "world"));
        final List<Object> results = new ArrayList<>();
        for (final Object c : Values.list(args.get("changes"))) {
            final Map<String, Object> change = Values.map(c);
            final Block block = world.getBlockAt(Values.i32(change, "x", 0), Values.i32(change, "y", 0), Values.i32(change, "z", 0));
            final org.bukkit.block.BlockState state = HostBlockState.of((HostBlock) block, Values.str(change, "state"));
            final Event event = switch (Values.str(change, "event")) {
                case "grow" -> new org.bukkit.event.block.BlockGrowEvent(block, state);
                case "fade" -> new org.bukkit.event.block.BlockFadeEvent(block, state);
                case "form" -> new org.bukkit.event.block.BlockFormEvent(block, state);
                case "spread" -> new org.bukkit.event.block.BlockSpreadEvent(block, this.at(world, change.get("from")), state);
                case "decay" -> new org.bukkit.event.block.LeavesDecayEvent(block);
                case "moisture" -> new org.bukkit.event.block.MoistureChangeEvent(block, state);
                default -> throw new IllegalArgumentException("unknown block change " + change.get("event"));
            };
            final boolean allowed = event.callEvent();
            results.add(Values.of("cancelled", !allowed, "state", state.getBlockData().getAsString()));
        }
        return Values.of("results", results);
    }

    /**
     * {@code PlayerLoginEvent}, which Paper still fires for its listeners, with the result of
     * the server's own checks: the result and message as plugins leave them. The player joins
     * as the same object, as in Paper.
     */
    private Object login(final Map<String, Object> args) {
        final HostPlayer player = this.server.loggingIn(Values.map(args.get("player")));
        InetAddress address;
        try {
            address = InetAddress.getByName(Values.str(args, "address"));
        } catch (final java.net.UnknownHostException e) {
            address = InetAddress.getLoopbackAddress();
        }
        final org.bukkit.event.player.PlayerLoginEvent event = new org.bukkit.event.player.PlayerLoginEvent(player, Values.str(args, "hostname", ""), address,
            org.bukkit.event.player.PlayerLoginEvent.Result.valueOf(Values.str(args, "result", "ALLOWED")), Components.fromJson(args.get("message")), address);
        event.callEvent();
        if (event.getResult() != org.bukkit.event.player.PlayerLoginEvent.Result.ALLOWED) {
            this.server.forgetLogin(player.getUniqueId());
        }
        return Values.of("result", event.getResult().name(), "message", Components.toJson(event.kickMessage()));
    }

    private Object join(final Map<String, Object> args) {
        final HostPlayer player = this.server.addPlayer(Values.map(args.get("player")));
        final PlayerJoinEvent event = new PlayerJoinEvent(player, Components.fromJson(args.get("message")));
        event.callEvent();
        return Values.of("message", Components.toJson(event.joinMessage()));
    }

    private Object quit(final Map<String, Object> args) {
        final UUID uuid = Values.uuid(args, "uuid");
        final HostPlayer player = this.server.player(uuid);
        if (player == null) {
            return Values.of("message", args.get("message"));
        }
        final String reason = Values.str(args, "reason", "").toLowerCase(Locale.ROOT);
        final PlayerQuitEvent.QuitReason quitReason = reason.contains("timed out") || reason.contains("timeout")
            ? PlayerQuitEvent.QuitReason.TIMED_OUT
            : reason.contains("kick") ? PlayerQuitEvent.QuitReason.KICKED : PlayerQuitEvent.QuitReason.DISCONNECTED;
        final PlayerQuitEvent event = new PlayerQuitEvent(player, Components.fromJson(args.get("message")), quitReason);
        try {
            event.callEvent();
        } finally {
            this.server.removePlayer(uuid);
        }
        return Values.of("message", Components.toJson(event.quitMessage()));
    }

    /** Chat runs on a bridge thread, as Paper's AsyncChatEvent runs off the main thread. */
    private Object chat(final Map<String, Object> args) {
        final HostPlayer player = this.player(args);
        String message = Values.str(args, "message");
        final Set<Player> recipients = new LinkedHashSet<>(this.server.getOnlinePlayers());
        String legacyFormat = null;
        if (AsyncPlayerChatEvent.getHandlerList().getRegisteredListeners().length > 0) {
            final AsyncPlayerChatEvent legacy = new AsyncPlayerChatEvent(true, player, message, recipients);
            legacy.callEvent();
            if (legacy.isCancelled()) {
                return Values.of("cancelled", true);
            }
            message = legacy.getMessage();
            if (!legacy.getFormat().equals("<%1$s> %2$s")) {
                legacyFormat = legacy.getFormat();
            }
        }
        final Set<Audience> viewers = new LinkedHashSet<>(recipients);
        viewers.add(this.server.getConsoleSender());
        final Component text = Component.text(message);
        final String formatted = legacyFormat;
        final ChatRenderer renderer = formatted == null ? ChatRenderer.defaultRenderer()
            : ChatRenderer.viewerUnaware((source, displayName, msg) -> Components.legacy(String.format(formatted, Components.toLegacy(displayName), Components.toLegacy(msg))));
        final AsyncChatEvent event = new AsyncChatEvent(true, player, viewers, renderer, text, text, signed(player, message));
        event.callEvent();
        if (event.isCancelled()) {
            return Values.of("cancelled", true);
        }
        final List<Object> deliveries = new ArrayList<>();
        for (Audience viewer : event.viewers()) {
            final Component rendered = event.renderer().render(player, player.displayName(), event.message(), viewer);
            final String id = viewer instanceof Player p ? p.getUniqueId().toString() : viewer == this.server.getConsoleSender() ? "console" : null;
            if (id != null) {
                deliveries.add(Values.of("viewer", id, "message", Components.toJson(rendered)));
            } else {
                viewer.sendMessage(rendered);
            }
        }
        return Values.of("cancelled", false, "deliveries", deliveries);
    }

    private static SignedMessage signed(final Player player, final String message) {
        final Instant now = Instant.now();
        return new SignedMessage() {
            @Override
            public Instant timestamp() {
                return now;
            }

            @Override
            public long salt() {
                return 0;
            }

            @Override
            public @Nullable Signature signature() {
                return null;
            }

            @Override
            public @Nullable Component unsignedContent() {
                return null;
            }

            @Override
            public String message() {
                return message;
            }

            @Override
            public Identity identity() {
                return Identity.identity(player.getUniqueId());
            }
        };
    }

    private Object ping(final Map<String, Object> args) {
        final InetSocketAddress address = new InetSocketAddress(parseHost(Values.str(args, "address", "127.0.0.1")), 0);
        final int protocol = Values.i32(args, "protocol", 0);
        final StatusClient client = new StatusClient() {
            @Override
            public InetSocketAddress getAddress() {
                return address;
            }

            @Override
            public SocketAddress getSocketAddress() {
                return address;
            }

            @Override
            public int getProtocolVersion() {
                return protocol;
            }

            @Override
            public @Nullable InetSocketAddress getVirtualHost() {
                return null;
            }
        };
        final PaperServerListPingEvent event = new PaperServerListPingEvent(client, Components.fromJson(args.get("motd")),
            Values.i32(args, "online", 0), Values.i32(args, "max", 0), Values.str(args, "version", ""), protocol, null);
        event.callEvent();
        final List<Object> sample = new ArrayList<>();
        for (PaperServerListPingEvent.ListedPlayerInfo info : event.getListedPlayers()) {
            sample.add(Values.of("name", info.name(), "uuid", info.id().toString()));
        }
        return Values.of("cancelled", event.isCancelled(), "motd", Components.toJson(event.motd()), "online", event.getNumPlayers(),
            "max", event.getMaxPlayers(), "version", event.getVersion(), "protocol", event.getProtocolVersion(),
            "hide_players", event.shouldHidePlayers(), "sample", sample);
    }

    private Object preLogin(final Map<String, Object> args) {
        final InetAddress address = parseHost(Values.str(args, "address", "127.0.0.1"));
        final AsyncPlayerPreLoginEvent event = new AsyncPlayerPreLoginEvent(Values.str(args, "name"), address, Values.uuid(args, "uuid"), false);
        event.callEvent();
        return Values.of("result", event.getLoginResult().name(), "message", Components.toJson(event.kickMessage()));
    }

    private static InetAddress parseHost(final String host) {
        try {
            return InetAddress.getByName(host.replace("[", "").replace("]", ""));
        } catch (java.net.UnknownHostException e) {
            return InetAddress.getLoopbackAddress();
        }
    }
}
