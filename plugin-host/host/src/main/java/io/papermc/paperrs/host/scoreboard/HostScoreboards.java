package io.papermc.paperrs.host.scoreboard;

import com.google.common.base.Preconditions;
import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.HostProxies;
import io.papermc.paperrs.host.bridge.Values;
import java.lang.ref.Cleaner;
import java.lang.ref.WeakReference;
import java.util.ArrayList;
import java.util.Collection;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import net.kyori.adventure.audience.Audience;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import net.kyori.adventure.text.format.TextColor;
import org.bukkit.ChatColor;
import org.bukkit.OfflinePlayer;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.Entity;
import org.bukkit.entity.Player;
import org.bukkit.scoreboard.Criteria;
import org.bukkit.scoreboard.DisplaySlot;
import org.bukkit.scoreboard.NameTagVisibility;
import org.bukkit.scoreboard.Objective;
import org.bukkit.scoreboard.RenderType;
import org.bukkit.scoreboard.Score;
import org.bukkit.scoreboard.Scoreboard;
import org.bukkit.scoreboard.ScoreboardManager;
import org.bukkit.scoreboard.Team;
import org.jetbrains.annotations.Nullable;

/**
 * Scoreboards (Paper's {@code CraftScoreboard*} classes). The boards live on the Rust server
 * ({@code server::scoreboard}), which also sends them to the players viewing them; the objects
 * here are handles that ask the server. Each board has one handle object while plugins hold it;
 * once it is garbage the server is told, and forgets the board when nobody views it.
 */
public final class HostScoreboards {
    private static final Cleaner CLEANER = Cleaner.create();
    /** The display slot ids in the server's order. */
    private static final List<String> SLOTS = List.of("list", "sidebar", "below_name",
        "sidebar.team.black", "sidebar.team.dark_blue", "sidebar.team.dark_green", "sidebar.team.dark_aqua", "sidebar.team.dark_red", "sidebar.team.dark_purple",
        "sidebar.team.gold", "sidebar.team.gray", "sidebar.team.dark_gray", "sidebar.team.blue", "sidebar.team.green", "sidebar.team.aqua", "sidebar.team.red",
        "sidebar.team.light_purple", "sidebar.team.yellow", "sidebar.team.white");
    private static final Map<Long, WeakReference<Scoreboard>> BOARDS = new ConcurrentHashMap<>();
    private static final Map<String, Criteria> CRITERIA = new ConcurrentHashMap<>();

    private HostScoreboards() {
    }

    // --- manager ------------------------------------------------------------------------------

    public static ScoreboardManager manager(final CraftServer server) {
        return HostProxies.create(new Manager(server), ScoreboardManager.class);
    }

    public static final class Manager {
        private final CraftServer server;

        Manager(final CraftServer server) {
            this.server = server;
        }

        public Scoreboard getMainScoreboard() {
            return board(this.server, 0);
        }

        public Scoreboard getNewScoreboard() {
            final long id = Values.i64(Values.map(this.server.call("scoreboard.new", Values.of())), "board");
            return board(this.server, id);
        }

        @Override
        public boolean equals(final Object o) {
            return o instanceof Manager;
        }

        @Override
        public int hashCode() {
            return 1;
        }
    }

    /** The handle of a board, made once while plugins hold it. */
    public static synchronized Scoreboard board(final CraftServer server, final long id) {
        final WeakReference<Scoreboard> known = BOARDS.get(id);
        final Scoreboard existing = known == null ? null : known.get();
        if (existing != null) {
            return existing;
        }
        final Scoreboard board = HostProxies.create(new Board(server, id), Scoreboard.class);
        if (id != 0) {
            if (known != null) {
                // The handle was collected but players still view the board: keep it.
                server.send("scoreboard.retain", Values.of("board", id));
            }
            CLEANER.register(board, () -> server.send("scoreboard.release", Values.of("board", id)));
        }
        BOARDS.put(id, new WeakReference<>(board));
        return board;
    }

    public static long idOf(final Scoreboard scoreboard) {
        final Board board = HostProxies.implOf(scoreboard, Board.class);
        if (board == null) {
            throw new IllegalArgumentException("not a scoreboard made by Paper-RS: " + scoreboard);
        }
        return board.id;
    }

    /** Forget all handles (the plugin host is going away). */
    public static void clear() {
        BOARDS.clear();
    }

    // --- criteria -------------------------------------------------------------------------------

    public static Criteria criteria(final String name) {
        return CRITERIA.computeIfAbsent(name, n -> HostProxies.create(new HostCriteria(n), Criteria.class));
    }

    public static final class HostCriteria {
        private final String name;

        HostCriteria(final String name) {
            this.name = name;
        }

        public String getName() {
            return this.name;
        }

        public boolean isReadOnly() {
            return switch (this.name) {
                case "health", "food", "air", "armor", "level", "xp" -> true;
                default -> false;
            };
        }

        public RenderType getDefaultRenderType() {
            return this.name.equals("health") ? RenderType.HEARTS : RenderType.INTEGER;
        }

        @Override
        public boolean equals(final Object o) {
            return o instanceof HostCriteria c && c.name.equals(this.name);
        }

        @Override
        public int hashCode() {
            return this.name.hashCode();
        }

        @Override
        public String toString() {
            return "CraftCriteria{" + this.name + "}";
        }
    }

    // --- helpers --------------------------------------------------------------------------------

    static int slotId(final DisplaySlot slot) {
        final int index = SLOTS.indexOf(slot.getId());
        Preconditions.checkArgument(index >= 0, "Unknown display slot %s", slot);
        return index;
    }

    static @Nullable DisplaySlot slotOf(final int id) {
        if (id < 0 || id >= SLOTS.size()) {
            return null;
        }
        return DisplaySlot.NAMES.value(SLOTS.get(id));
    }

    static String entryOf(final Entity entity) {
        return entity instanceof Player player ? player.getName() : entity.getUniqueId().toString();
    }

    static String entryOf(final OfflinePlayer player) {
        final String name = player.getName();
        Preconditions.checkArgument(name != null, "the player has no known name");
        return name;
    }

    // --- scoreboard -----------------------------------------------------------------------------

    public static final class Board {
        final CraftServer server;
        final long id;

        Board(final CraftServer server, final long id) {
            this.server = server;
            this.id = id;
        }

        Object call(final String method, final Object... kv) {
            final Map<String, Object> args = Values.of(kv);
            args.put("board", this.id);
            return this.server.call(method, args);
        }

        Scoreboard self() {
            return board(this.server, this.id);
        }

        private Objective objective(final String name) {
            return HostProxies.create(new HostObjective(this, name), Objective.class);
        }

        private Team team(final String name) {
            return HostProxies.create(new HostTeam(this, name), Team.class);
        }

        public Objective registerNewObjective(final String name, final String criteria, final @Nullable Component displayName, final @Nullable RenderType renderType) {
            return this.registerNewObjective(name, criteria(criteria), displayName, renderType);
        }

        public Objective registerNewObjective(final String name, final Criteria criteria, final @Nullable Component displayName, final @Nullable RenderType renderType) {
            Preconditions.checkArgument(name != null, "Objective name cannot be null");
            Preconditions.checkArgument(criteria != null, "Criteria cannot be null");
            final RenderType render = renderType != null ? renderType : criteria.getDefaultRenderType();
            this.call("scoreboard.register_objective", "name", name, "criteria", criteria.getName(),
                "display_name", Components.toJson(displayName != null ? displayName : Component.text(name)), "render_type", render == RenderType.HEARTS ? 1 : 0);
            return this.objective(name);
        }

        public Objective registerNewObjective(final String name, final String criteria, final String displayName, final RenderType renderType) {
            return this.registerNewObjective(name, criteria, Components.legacy(displayName), renderType);
        }

        public @Nullable Objective getObjective(final String name) {
            return this.call("scoreboard.objective", "name", name) instanceof Map<?, ?> ? this.objective(name) : null;
        }

        public @Nullable Objective getObjective(final DisplaySlot slot) {
            final int id = slotId(slot);
            for (Object o : Values.list(this.call("scoreboard.objectives"))) {
                final Map<String, Object> info = Values.map(o);
                for (Object s : Values.list(info.get("display_slots"))) {
                    if (((Number) s).intValue() == id) {
                        return this.objective(Values.str(info, "name"));
                    }
                }
            }
            return null;
        }

        public Set<Objective> getObjectives() {
            final Set<Objective> out = new LinkedHashSet<>();
            for (Object o : Values.list(this.call("scoreboard.objectives"))) {
                out.add(this.objective(Values.str(Values.map(o), "name")));
            }
            return out;
        }

        public Set<Objective> getObjectivesByCriteria(final String criteria) {
            final Set<Objective> out = new LinkedHashSet<>();
            for (Object o : Values.list(this.call("scoreboard.objectives"))) {
                final Map<String, Object> info = Values.map(o);
                if (Values.str(info, "criteria", "").equals(criteria)) {
                    out.add(this.objective(Values.str(info, "name")));
                }
            }
            return out;
        }

        public Set<Objective> getObjectivesByCriteria(final Criteria criteria) {
            return this.getObjectivesByCriteria(criteria.getName());
        }

        public Set<Score> getScores(final String entry) {
            final Set<Score> out = new LinkedHashSet<>();
            for (Object s : Values.list(this.call("scoreboard.entry_scores", "entry", entry))) {
                out.add(HostProxies.create(new HostScore(new HostObjective(this, Values.str(Values.map(s), "objective")), entry), Score.class));
            }
            return out;
        }

        public Set<Score> getScores(final OfflinePlayer player) {
            return this.getScores(entryOf(player));
        }

        public Set<Score> getScoresFor(final Entity entity) {
            return this.getScores(entryOf(entity));
        }

        public void resetScores(final String entry) {
            this.call("scoreboard.reset_scores", "entry", entry);
        }

        public void resetScores(final OfflinePlayer player) {
            this.resetScores(entryOf(player));
        }

        public void resetScoresFor(final Entity entity) {
            this.resetScores(entryOf(entity));
        }

        public @Nullable Team getEntryTeam(final String entry) {
            final Object team = this.call("scoreboard.entry_team", "entry", entry);
            return team instanceof Map<?, ?> t ? this.team(Values.str(Values.map(t), "name")) : null;
        }

        public @Nullable Team getPlayerTeam(final OfflinePlayer player) {
            return this.getEntryTeam(entryOf(player));
        }

        public @Nullable Team getEntityTeam(final Entity entity) {
            return this.getEntryTeam(entryOf(entity));
        }

        public @Nullable Team getTeam(final String name) {
            return this.call("scoreboard.team", "name", name) instanceof Map<?, ?> ? this.team(name) : null;
        }

        public Set<Team> getTeams() {
            final Set<Team> out = new LinkedHashSet<>();
            for (Object t : Values.list(this.call("scoreboard.teams"))) {
                out.add(this.team(Values.str(Values.map(t), "name")));
            }
            return out;
        }

        public Team registerNewTeam(final String name) {
            Preconditions.checkArgument(name != null, "Team name cannot be null");
            this.call("scoreboard.register_team", "name", name);
            return this.team(name);
        }

        public Set<String> getEntries() {
            final Set<String> out = new LinkedHashSet<>();
            for (Object e : Values.list(this.call("scoreboard.entries"))) {
                out.add(String.valueOf(e));
            }
            return out;
        }

        public Set<OfflinePlayer> getPlayers() {
            final Set<OfflinePlayer> out = new LinkedHashSet<>();
            for (String entry : this.getEntries()) {
                out.add(this.server.getOfflinePlayer(entry));
            }
            return out;
        }

        public void clearSlot(final DisplaySlot slot) {
            this.call("scoreboard.set_display", "slot", slotId(slot));
        }

        @Override
        public boolean equals(final Object o) {
            return o instanceof Board b && b.id == this.id;
        }

        @Override
        public int hashCode() {
            return Long.hashCode(this.id);
        }

        @Override
        public String toString() {
            return "CraftScoreboard{" + this.id + "}";
        }
    }

    // --- objective ------------------------------------------------------------------------------

    public static final class HostObjective {
        final Board board;
        final String name;

        HostObjective(final Board board, final String name) {
            this.board = board;
            this.name = name;
        }

        Map<String, Object> info() {
            final Object info = this.board.call("scoreboard.objective", "name", this.name);
            if (!(info instanceof Map<?, ?>)) {
                throw new IllegalStateException("Unregistered scoreboard component");
            }
            return Values.map(info);
        }

        public String getName() {
            return this.name;
        }

        public Component displayName() {
            return Components.fromJson(this.info().get("display_name"));
        }

        public void displayName(final @Nullable Component name) {
            this.board.call("scoreboard.update_objective", "name", this.name, "display_name", Components.toJson(name == null ? Component.empty() : name));
        }

        public String getDisplayName() {
            return Components.toLegacy(this.displayName());
        }

        public void setDisplayName(final String name) {
            this.displayName(Components.legacy(name));
        }

        public String getCriteria() {
            return Values.str(this.info(), "criteria", "dummy");
        }

        public Criteria getTrackedCriteria() {
            return criteria(this.getCriteria());
        }

        public boolean isModifiable() {
            return !this.getTrackedCriteria().isReadOnly();
        }

        public Scoreboard getScoreboard() {
            return this.board.self();
        }

        public void unregister() {
            this.info();
            this.board.call("scoreboard.unregister_objective", "name", this.name);
        }

        public void setDisplaySlot(final @Nullable DisplaySlot slot) {
            this.info();
            final DisplaySlot current = this.getDisplaySlot();
            if (current != null && current != slot) {
                this.board.call("scoreboard.set_display", "slot", slotId(current));
            }
            if (slot != null) {
                this.board.call("scoreboard.set_display", "slot", slotId(slot), "objective", this.name);
            }
        }

        public @Nullable DisplaySlot getDisplaySlot() {
            final List<Object> slots = Values.list(this.info().get("display_slots"));
            return slots.isEmpty() ? null : slotOf(((Number) slots.getFirst()).intValue());
        }

        public void setRenderType(final RenderType type) {
            this.board.call("scoreboard.update_objective", "name", this.name, "render_type", type == RenderType.HEARTS ? 1 : 0);
        }

        public RenderType getRenderType() {
            return Values.i32(this.info(), "render_type", 0) == 1 ? RenderType.HEARTS : RenderType.INTEGER;
        }

        public Score getScore(final String entry) {
            Preconditions.checkArgument(entry != null, "Entry cannot be null");
            this.info();
            return HostProxies.create(new HostScore(this, entry), Score.class);
        }

        public Score getScore(final OfflinePlayer player) {
            return this.getScore(entryOf(player));
        }

        public Score getScoreFor(final Entity entity) {
            return this.getScore(entryOf(entity));
        }

        @Override
        public boolean equals(final Object o) {
            return o instanceof HostObjective other && other.board.equals(this.board) && other.name.equals(this.name);
        }

        @Override
        public int hashCode() {
            return this.board.hashCode() * 31 + this.name.hashCode();
        }

        @Override
        public String toString() {
            return "CraftObjective{" + this.name + "}";
        }
    }

    // --- score ----------------------------------------------------------------------------------

    public static final class HostScore {
        final HostObjective objective;
        final String entry;

        HostScore(final HostObjective objective, final String entry) {
            this.objective = objective;
            this.entry = entry;
        }

        private Map<String, Object> info() {
            return Values.map(this.objective.board.call("scoreboard.score", "objective", this.objective.name, "entry", this.entry));
        }

        public String getEntry() {
            return this.entry;
        }

        public OfflinePlayer getPlayer() {
            return this.objective.board.server.getOfflinePlayer(this.entry);
        }

        public Objective getObjective() {
            return HostProxies.create(this.objective, Objective.class);
        }

        public Scoreboard getScoreboard() {
            return this.objective.board.self();
        }

        public int getScore() {
            return Values.i32(this.info(), "value", 0);
        }

        public void setScore(final int score) {
            this.objective.board.call("scoreboard.set_score", "objective", this.objective.name, "entry", this.entry, "value", score);
        }

        public boolean isScoreSet() {
            return Values.bool(this.info(), "set", false);
        }

        public void resetScore() {
            this.objective.board.call("scoreboard.reset_scores", "entry", this.entry, "objective", this.objective.name);
        }

        /** Whether {@code /trigger} accepts this score (a set, unlocked score of a trigger objective). */
        public boolean isTriggerable() {
            if (!"trigger".equals(this.objective.getCriteria())) {
                return false;
            }
            final Map<String, Object> info = this.info();
            return Values.bool(info, "set", false) && !Values.bool(info, "locked", true);
        }

        public void setTriggerable(final boolean triggerable) {
            if (!"trigger".equals(this.objective.getCriteria())) {
                throw new IllegalArgumentException("the criteria isn't 'trigger'");
            }
            this.objective.board.call("scoreboard.set_score", "objective", this.objective.name, "entry", this.entry, "locked", !triggerable);
        }

        public @Nullable Component customName() {
            return Components.fromJson(this.info().get("display_name"));
        }

        public void customName(final @Nullable Component name) {
            if (name == null) {
                this.objective.board.call("scoreboard.set_score", "objective", this.objective.name, "entry", this.entry, "clear_display_name", true);
            } else {
                this.objective.board.call("scoreboard.set_score", "objective", this.objective.name, "entry", this.entry, "display_name", Components.toJson(name));
            }
        }

        @Override
        public boolean equals(final Object o) {
            return o instanceof HostScore other && other.objective.equals(this.objective) && other.entry.equals(this.entry);
        }

        @Override
        public int hashCode() {
            return this.objective.hashCode() * 31 + this.entry.hashCode();
        }

        @Override
        public String toString() {
            return "CraftScore{" + this.objective.name + ", " + this.entry + "}";
        }
    }

    // --- team -----------------------------------------------------------------------------------

    public static final class HostTeam {
        final Board board;
        final String name;

        HostTeam(final Board board, final String name) {
            this.board = board;
            this.name = name;
        }

        private Map<String, Object> info() {
            final Object info = this.board.call("scoreboard.team", "name", this.name);
            if (!(info instanceof Map<?, ?>)) {
                throw new IllegalStateException("Unregistered scoreboard component");
            }
            return Values.map(info);
        }

        private void update(final Object... kv) {
            final Object[] args = new Object[kv.length + 2];
            args[0] = "name";
            args[1] = this.name;
            System.arraycopy(kv, 0, args, 2, kv.length);
            this.board.call("scoreboard.update_team", args);
        }

        public String getName() {
            return this.name;
        }

        public Component displayName() {
            return Components.fromJson(this.info().get("display_name"));
        }

        public void displayName(final @Nullable Component name) {
            this.update("display_name", Components.toJson(name == null ? Component.empty() : name));
        }

        public Component prefix() {
            return Components.fromJson(this.info().get("prefix"));
        }

        public void prefix(final @Nullable Component prefix) {
            this.update("prefix", Components.toJson(prefix == null ? Component.empty() : prefix));
        }

        public Component suffix() {
            return Components.fromJson(this.info().get("suffix"));
        }

        public void suffix(final @Nullable Component suffix) {
            this.update("suffix", Components.toJson(suffix == null ? Component.empty() : suffix));
        }

        public boolean hasColor() {
            return this.info().get("color") instanceof Number;
        }

        public TextColor color() {
            final Object color = this.info().get("color");
            if (!(color instanceof Number n)) {
                throw new IllegalStateException("Team colors must have hex values");
            }
            return colors().get(n.intValue());
        }

        public void color(final @Nullable NamedTextColor color) {
            if (color == null) {
                this.update("clear_color", true);
            } else {
                this.update("color", (long) colors().indexOf(color));
            }
        }

        private static List<NamedTextColor> colors() {
            return List.of(NamedTextColor.BLACK, NamedTextColor.DARK_BLUE, NamedTextColor.DARK_GREEN, NamedTextColor.DARK_AQUA, NamedTextColor.DARK_RED,
                NamedTextColor.DARK_PURPLE, NamedTextColor.GOLD, NamedTextColor.GRAY, NamedTextColor.DARK_GRAY, NamedTextColor.BLUE, NamedTextColor.GREEN,
                NamedTextColor.AQUA, NamedTextColor.RED, NamedTextColor.LIGHT_PURPLE, NamedTextColor.YELLOW, NamedTextColor.WHITE);
        }

        public String getDisplayName() {
            return Components.toLegacy(this.displayName());
        }

        public void setDisplayName(final String name) {
            this.displayName(Components.legacy(name));
        }

        public String getPrefix() {
            return Components.toLegacy(this.prefix());
        }

        public void setPrefix(final String prefix) {
            this.prefix(Components.legacy(prefix));
        }

        public String getSuffix() {
            return Components.toLegacy(this.suffix());
        }

        public void setSuffix(final String suffix) {
            this.suffix(Components.legacy(suffix));
        }

        @SuppressWarnings("deprecation")
        public ChatColor getColor() {
            final Object color = this.info().get("color");
            return color instanceof Number n ? ChatColor.values()[n.intValue()] : ChatColor.RESET;
        }

        @SuppressWarnings("deprecation")
        public void setColor(final ChatColor color) {
            if (color == null || color == ChatColor.RESET || !color.isColor()) {
                this.update("clear_color", true);
            } else {
                this.update("color", (long) color.ordinal());
            }
        }

        public boolean allowFriendlyFire() {
            return Values.bool(this.info(), "friendly_fire", true);
        }

        public void setAllowFriendlyFire(final boolean enabled) {
            this.update("friendly_fire", enabled);
        }

        public boolean canSeeFriendlyInvisibles() {
            return Values.bool(this.info(), "see_friendly_invisibles", true);
        }

        public void setCanSeeFriendlyInvisibles(final boolean enabled) {
            this.update("see_friendly_invisibles", enabled);
        }

        @SuppressWarnings("deprecation")
        public NameTagVisibility getNameTagVisibility() {
            return switch (Values.i32(this.info(), "name_tag_visibility", 0)) {
                case 1 -> NameTagVisibility.NEVER;
                case 2 -> NameTagVisibility.HIDE_FOR_OTHER_TEAMS;
                case 3 -> NameTagVisibility.HIDE_FOR_OWN_TEAM;
                default -> NameTagVisibility.ALWAYS;
            };
        }

        @SuppressWarnings("deprecation")
        public void setNameTagVisibility(final NameTagVisibility visibility) {
            this.update("name_tag_visibility", (long) visibility.ordinal());
        }

        public Team.OptionStatus getOption(final Team.Option option) {
            final String key = switch (option) {
                case NAME_TAG_VISIBILITY -> "name_tag_visibility";
                case DEATH_MESSAGE_VISIBILITY -> "death_message_visibility";
                case COLLISION_RULE -> "collision_rule";
            };
            return Team.OptionStatus.values()[Values.i32(this.info(), key, 0)];
        }

        public void setOption(final Team.Option option, final Team.OptionStatus status) {
            final String key = switch (option) {
                case NAME_TAG_VISIBILITY -> "name_tag_visibility";
                case DEATH_MESSAGE_VISIBILITY -> "death_message_visibility";
                case COLLISION_RULE -> "collision_rule";
            };
            this.update(key, (long) status.ordinal());
        }

        public Set<String> getEntries() {
            final Set<String> out = new LinkedHashSet<>();
            for (Object e : Values.list(this.info().get("entries"))) {
                out.add(String.valueOf(e));
            }
            return out;
        }

        public Set<OfflinePlayer> getPlayers() {
            final Set<OfflinePlayer> out = new LinkedHashSet<>();
            for (String entry : this.getEntries()) {
                out.add(this.board.server.getOfflinePlayer(entry));
            }
            return out;
        }

        public int getSize() {
            return this.getEntries().size();
        }

        public Scoreboard getScoreboard() {
            return this.board.self();
        }

        public void addEntry(final String entry) {
            this.addEntries(List.of(entry));
        }

        public void addEntries(final Collection<String> entries) {
            this.info();
            this.board.call("scoreboard.add_entries", "team", this.name, "entries", new ArrayList<Object>(entries));
        }

        public void addPlayer(final OfflinePlayer player) {
            this.addEntry(entryOf(player));
        }

        public void addEntity(final Entity entity) {
            this.addEntry(entryOf(entity));
        }

        public void addEntities(final Collection<Entity> entities) {
            final List<String> entries = new ArrayList<>();
            for (Entity entity : entities) {
                entries.add(entryOf(entity));
            }
            this.addEntries(entries);
        }

        public boolean removeEntry(final String entry) {
            return this.removeEntries(List.of(entry));
        }

        public boolean removeEntries(final Collection<String> entries) {
            this.info();
            return !Values.list(this.board.call("scoreboard.remove_entries", "team", this.name, "entries", new ArrayList<Object>(entries))).isEmpty();
        }

        public boolean removePlayer(final OfflinePlayer player) {
            return this.removeEntry(entryOf(player));
        }

        public boolean removeEntity(final Entity entity) {
            return this.removeEntry(entryOf(entity));
        }

        public boolean removeEntities(final Collection<Entity> entities) {
            final List<String> entries = new ArrayList<>();
            for (Entity entity : entities) {
                entries.add(entryOf(entity));
            }
            return this.removeEntries(entries);
        }

        public boolean hasEntry(final String entry) {
            return this.getEntries().contains(entry);
        }

        public boolean hasPlayer(final OfflinePlayer player) {
            return this.hasEntry(entryOf(player));
        }

        public boolean hasEntity(final Entity entity) {
            return this.hasEntry(entryOf(entity));
        }

        public void unregister() {
            this.board.call("scoreboard.unregister_team", "name", this.name);
        }

        /** {@code ForwardingAudience}: the online players in the team. */
        public Iterable<? extends Audience> audiences() {
            final List<Audience> out = new ArrayList<>();
            for (String entry : this.getEntries()) {
                final Player player = this.board.server.getPlayerExact(entry);
                if (player != null) {
                    out.add(player);
                }
            }
            return out;
        }

        @Override
        public boolean equals(final Object o) {
            return o instanceof HostTeam other && other.board.equals(this.board) && other.name.equals(this.name);
        }

        @Override
        public int hashCode() {
            return this.board.hashCode() * 31 + this.name.hashCode();
        }

        @Override
        public String toString() {
            return "CraftTeam{" + this.name + "}";
        }
    }
}
