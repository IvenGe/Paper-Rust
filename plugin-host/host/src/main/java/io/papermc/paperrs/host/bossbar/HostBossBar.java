package io.papermc.paperrs.host.bossbar;

import java.util.EnumSet;
import java.util.List;
import java.util.Set;
import net.kyori.adventure.bossbar.BossBar;
import net.kyori.adventure.text.serializer.legacy.LegacyComponentSerializer;
import org.bukkit.NamespacedKey;
import org.bukkit.boss.BarColor;
import org.bukkit.boss.BarFlag;
import org.bukkit.boss.BarStyle;
import org.bukkit.boss.KeyedBossBar;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.Player;
import org.jetbrains.annotations.Nullable;

/**
 * Bukkit's {@code BossBar} ({@code Server#createBossBar}), built on an Adventure bar as Paper's
 * {@code CraftBossBar} is. Hidden bars keep their players and show again to them.
 */
public final class HostBossBar implements KeyedBossBar {
    private final CraftServer server;
    private final @Nullable NamespacedKey key;
    private final BossBar bar;
    private final java.util.List<Player> players = new java.util.ArrayList<>();
    private final Set<BarFlag> flags = EnumSet.noneOf(BarFlag.class);
    private String title;
    private BarStyle style;
    private boolean visible = true;

    public HostBossBar(final CraftServer server, final @Nullable NamespacedKey key, final String title, final BarColor color, final BarStyle style, final BarFlag... flags) {
        this.server = server;
        this.key = key;
        this.title = title == null ? "" : title;
        this.style = style;
        this.bar = BossBar.bossBar(LegacyComponentSerializer.legacySection().deserialize(this.title), 1.0f, color(color), overlay(style));
        for (final BarFlag flag : flags) {
            this.addFlag(flag);
        }
    }

    private static BossBar.Color color(final BarColor color) {
        return switch (color) {
            case PINK -> BossBar.Color.PINK;
            case BLUE -> BossBar.Color.BLUE;
            case RED -> BossBar.Color.RED;
            case GREEN -> BossBar.Color.GREEN;
            case YELLOW -> BossBar.Color.YELLOW;
            case PURPLE -> BossBar.Color.PURPLE;
            case WHITE -> BossBar.Color.WHITE;
        };
    }

    private static BossBar.Overlay overlay(final BarStyle style) {
        return switch (style) {
            case SOLID -> BossBar.Overlay.PROGRESS;
            case SEGMENTED_6 -> BossBar.Overlay.NOTCHED_6;
            case SEGMENTED_10 -> BossBar.Overlay.NOTCHED_10;
            case SEGMENTED_12 -> BossBar.Overlay.NOTCHED_12;
            case SEGMENTED_20 -> BossBar.Overlay.NOTCHED_20;
        };
    }

    private static BossBar.Flag flag(final BarFlag flag) {
        return switch (flag) {
            case DARKEN_SKY -> BossBar.Flag.DARKEN_SCREEN;
            case PLAY_BOSS_MUSIC -> BossBar.Flag.PLAY_BOSS_MUSIC;
            case CREATE_FOG -> BossBar.Flag.CREATE_WORLD_FOG;
        };
    }

    @Override
    public NamespacedKey getKey() {
        if (this.key == null) {
            throw new IllegalStateException("This boss bar has no key; it was not made with Server#createBossBar(NamespacedKey, ...)");
        }
        return this.key;
    }

    @Override
    public String getTitle() {
        return this.title;
    }

    @Override
    public void setTitle(final @Nullable String title) {
        this.title = title == null ? "" : title;
        this.bar.name(LegacyComponentSerializer.legacySection().deserialize(this.title));
    }

    @Override
    public BarColor getColor() {
        return BarColor.valueOf(this.bar.color().name());
    }

    @Override
    public void setColor(final BarColor color) {
        this.bar.color(color(color));
    }

    @Override
    public BarStyle getStyle() {
        return this.style;
    }

    @Override
    public void setStyle(final BarStyle style) {
        this.style = style;
        this.bar.overlay(overlay(style));
    }

    @Override
    public void removeFlag(final BarFlag flag) {
        this.flags.remove(flag);
        this.bar.removeFlag(flag(flag));
    }

    @Override
    public void addFlag(final BarFlag flag) {
        this.flags.add(flag);
        this.bar.addFlag(flag(flag));
    }

    @Override
    public boolean hasFlag(final BarFlag flag) {
        return this.flags.contains(flag);
    }

    @Override
    public void setProgress(final double progress) {
        if (progress < 0.0 || progress > 1.0) {
            throw new IllegalArgumentException("Progress must be between 0.0 and 1.0 (" + progress + ")");
        }
        this.bar.progress((float) progress);
    }

    @Override
    public double getProgress() {
        return this.bar.progress();
    }

    @Override
    public void addPlayer(final Player player) {
        if (!this.players.contains(player)) {
            this.players.add(player);
        }
        if (this.visible) {
            BossBars.show(this.server, player, this.bar);
        }
    }

    @Override
    public void removePlayer(final Player player) {
        this.players.remove(player);
        BossBars.hide(this.server, player, this.bar);
    }

    @Override
    public void removeAll() {
        for (final Player player : List.copyOf(this.players)) {
            this.removePlayer(player);
        }
    }

    @Override
    public List<Player> getPlayers() {
        return List.copyOf(this.players);
    }

    @Override
    public void setVisible(final boolean visible) {
        if (visible == this.visible) {
            return;
        }
        this.visible = visible;
        for (final Player player : this.players) {
            if (visible) {
                BossBars.show(this.server, player, this.bar);
            } else {
                BossBars.hide(this.server, player, this.bar);
            }
        }
    }

    @Override
    public boolean isVisible() {
        return this.visible;
    }

    @Override
    public void show() {
        this.setVisible(true);
    }

    @Override
    public void hide() {
        this.setVisible(false);
    }
}
