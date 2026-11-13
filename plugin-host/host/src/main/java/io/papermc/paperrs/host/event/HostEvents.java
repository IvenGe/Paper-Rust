package io.papermc.paperrs.host.event;

import com.destroystokyo.paper.event.server.PaperServerListPingEvent;
import com.destroystokyo.paper.network.StatusClient;
import io.papermc.paper.chat.ChatRenderer;
import io.papermc.paper.event.player.AsyncChatEvent;
import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.entity.HostPlayer;
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
        GATED.put("BlockPlaceEvent", BlockPlaceEvent.class);
        GATED.put("PlayerMoveEvent", PlayerMoveEvent.class);
        GATED.put("PlayerGameModeChangeEvent", PlayerGameModeChangeEvent.class);
        GATED.put("AsyncChatEvent", AsyncChatEvent.class);
        GATED.put("AsyncPlayerChatEvent", AsyncPlayerChatEvent.class);
    }

    private final CraftServer server;
    private final Map<String, HandlerList> handlerLists = new LinkedHashMap<>();

    public HostEvents(final CraftServer server) {
        this.server = server;
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
            if (list.getRegisteredListeners().length > 0) {
                out.add(name);
            }
        });
        return out;
    }

    private HostPlayer player(final Map<String, Object> args) {
        final HostPlayer player = this.server.player(Values.uuid(args, "uuid"));
        if (player == null) {
            throw new IllegalArgumentException("player " + args.get("uuid") + " is not online");
        }
        return player;
    }

    private Location location(final World world, final Map<String, Object> loc) {
        return new Location(world, Values.f64(loc, "x", 0), Values.f64(loc, "y", 0), Values.f64(loc, "z", 0), (float) Values.f64(loc, "yaw", 0), (float) Values.f64(loc, "pitch", 0));
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
            case "event.block_break" -> {
                final BlockBreakEvent event = new BlockBreakEvent(this.block(args), this.player(args));
                event.callEvent();
                yield Values.of("cancelled", event.isCancelled());
            }
            case "event.block_place" -> {
                final Block placed = this.block(args);
                final List<Object> against = Values.list(args.get("against"));
                final Block clicked = placed.getWorld().getBlockAt(((Number) against.get(0)).intValue(), ((Number) against.get(1)).intValue(), ((Number) against.get(2)).intValue());
                final BlockPlaceEvent event = new BlockPlaceEvent(placed, null, clicked, null, this.player(args), true,
                    Values.bool(args, "offhand", false) ? EquipmentSlot.OFF_HAND : EquipmentSlot.HAND);
                event.callEvent();
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
            case "event.server_list_ping" -> this.ping(args);
            case "event.pre_login" -> this.preLogin(args);
            default -> throw new IllegalArgumentException("unknown event " + method);
        };
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
