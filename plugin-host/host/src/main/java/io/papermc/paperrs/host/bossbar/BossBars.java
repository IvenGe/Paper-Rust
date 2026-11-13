package io.papermc.paperrs.host.bossbar;

import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.bridge.Values;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.WeakHashMap;
import net.kyori.adventure.bossbar.BossBar;
import net.kyori.adventure.text.Component;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.Player;

/**
 * Adventure boss bars shown to players. Each bar gets a UUID the first time it is shown and a
 * listener that sends its changes to the server, which keeps its viewers.
 */
public final class BossBars {
    private static final Map<BossBar, UUID> IDS = new WeakHashMap<>();
    private static final Map<BossBar, List<Player>> VIEWERS = new WeakHashMap<>();

    private BossBars() {
    }

    private static int flags(final Set<BossBar.Flag> flags) {
        int out = 0;
        if (flags.contains(BossBar.Flag.DARKEN_SCREEN)) {
            out |= 1;
        }
        if (flags.contains(BossBar.Flag.PLAY_BOSS_MUSIC)) {
            out |= 2;
        }
        if (flags.contains(BossBar.Flag.CREATE_WORLD_FOG)) {
            out |= 4;
        }
        return out;
    }

    private static Map<String, Object> look(final UUID id, final BossBar bar) {
        return Values.of("id", id.toString(), "title", Components.toJson(bar.name()), "progress", (double) bar.progress(),
            "color", bar.color().name().toLowerCase(Locale.ROOT), "overlay", bar.overlay().name().toLowerCase(Locale.ROOT), "flags", flags(bar.flags()));
    }

    /** The id of a bar, registering it with the server (and listening to it) on first use. */
    private static synchronized UUID id(final CraftServer server, final BossBar bar) {
        final UUID known = IDS.get(bar);
        if (known != null) {
            return known;
        }
        final UUID id = UUID.randomUUID();
        IDS.put(bar, id);
        server.call("bossbar.update", look(id, bar));
        bar.addListener(new BossBar.Listener() {
            @Override
            public void bossBarNameChanged(final BossBar b, final Component oldName, final Component newName) {
                server.send("bossbar.update", Values.of("id", id.toString(), "title", Components.toJson(newName)));
            }

            @Override
            public void bossBarProgressChanged(final BossBar b, final float oldProgress, final float newProgress) {
                server.send("bossbar.update", Values.of("id", id.toString(), "progress", (double) newProgress));
            }

            @Override
            public void bossBarColorChanged(final BossBar b, final BossBar.Color oldColor, final BossBar.Color newColor) {
                server.send("bossbar.update", Values.of("id", id.toString(), "color", newColor.name().toLowerCase(Locale.ROOT)));
            }

            @Override
            public void bossBarOverlayChanged(final BossBar b, final BossBar.Overlay oldOverlay, final BossBar.Overlay newOverlay) {
                server.send("bossbar.update", Values.of("id", id.toString(), "overlay", newOverlay.name().toLowerCase(Locale.ROOT)));
            }

            @Override
            public void bossBarFlagsChanged(final BossBar b, final Set<BossBar.Flag> added, final Set<BossBar.Flag> removed) {
                server.send("bossbar.update", Values.of("id", id.toString(), "flags", flags(b.flags())));
            }
        });
        return id;
    }

    public static synchronized void show(final CraftServer server, final Player player, final BossBar bar) {
        final UUID id = id(server, bar);
        server.call("bossbar.show", Values.of("id", id.toString(), "player", player.getUniqueId().toString()));
        final List<Player> viewers = VIEWERS.computeIfAbsent(bar, b -> new ArrayList<>());
        if (!viewers.contains(player)) {
            viewers.add(player);
        }
    }

    public static synchronized void hide(final CraftServer server, final Player player, final BossBar bar) {
        final UUID id = IDS.get(bar);
        if (id == null) {
            return;
        }
        server.call("bossbar.hide", Values.of("id", id.toString(), "player", player.getUniqueId().toString()));
        final List<Player> viewers = VIEWERS.get(bar);
        if (viewers != null) {
            viewers.remove(player);
        }
    }

    /** The bars a player sees ({@code Audience#activeBossBars}). */
    public static synchronized List<BossBar> shownTo(final Player player) {
        final List<BossBar> out = new ArrayList<>();
        for (final Map.Entry<BossBar, List<Player>> e : VIEWERS.entrySet()) {
            if (e.getValue().contains(player)) {
                out.add(e.getKey());
            }
        }
        return out;
    }

    public static synchronized List<Player> viewers(final BossBar bar) {
        return List.copyOf(VIEWERS.getOrDefault(bar, List.of()));
    }
}
