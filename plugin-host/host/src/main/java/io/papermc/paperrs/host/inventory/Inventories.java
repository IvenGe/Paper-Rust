package io.papermc.paperrs.host.inventory;

import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.HostProxies;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.entity.HostPlayer;
import java.lang.ref.WeakReference;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import net.kyori.adventure.text.Component;
import org.bukkit.World;
import org.bukkit.block.Block;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.event.inventory.InventoryType;
import org.bukkit.inventory.CraftingInventory;
import org.bukkit.inventory.DoubleChestInventory;
import org.bukkit.inventory.FurnaceInventory;
import org.bukkit.inventory.Inventory;
import org.bukkit.inventory.InventoryHolder;
import org.bukkit.inventory.InventoryView;
import org.bukkit.inventory.PlayerInventory;
import org.jetbrains.annotations.Nullable;

/** Makes inventory objects for the server's inventory descriptors. */
public final class Inventories {
    /** Custom inventories by id, so the server's id always maps to the plugin's object. */
    private static final Map<Long, WeakReference<Inventory>> CUSTOM = new ConcurrentHashMap<>();

    private Inventories() {
    }

    private static Inventory proxy(final HostInventory impl, final Class<? extends Inventory> type) {
        return HostProxies.create(impl, type);
    }

    public static PlayerInventory player(final CraftServer server, final HostPlayer player) {
        final HostInventory impl = new HostInventory(server, Values.of("kind", "player", "uuid", player.getUniqueId().toString()), InventoryType.PLAYER, 41, null, null);
        return (PlayerInventory) proxy(impl, PlayerInventory.class);
    }

    public static Inventory enderChest(final CraftServer server, final HostPlayer player) {
        final HostInventory impl = new HostInventory(server, Values.of("kind", "ender", "uuid", player.getUniqueId().toString()), InventoryType.ENDER_CHEST, 27, null, null);
        return proxy(impl, Inventory.class);
    }

    public static CraftingInventory crafting(final CraftServer server, final HostPlayer player, final boolean workbench) {
        final HostInventory impl = new HostInventory(server, Values.of("kind", "crafting", "uuid", player.getUniqueId().toString()),
            workbench ? InventoryType.WORKBENCH : InventoryType.CRAFTING, workbench ? 10 : 5, null, null);
        return (CraftingInventory) proxy(impl, CraftingInventory.class);
    }

    /** The block a {@code block} descriptor names (its first position). */
    static Block blockOf(final CraftServer server, final Map<String, Object> ref) {
        final World world = server.getWorld(Values.str(ref, "world", "world"));
        return world.getBlockAt(Values.i32(ref, "x", 0), Values.i32(ref, "y", 0), Values.i32(ref, "z", 0));
    }

    /** One half of a double chest descriptor. */
    static Inventory half(final CraftServer server, final Map<String, Object> ref, final int index) {
        final List<Object> positions = Values.list(ref.get("positions"));
        final List<Object> pos = Values.list(positions.get(Math.min(index, positions.size() - 1)));
        return block(server, Values.of("kind", "block", "world", ref.get("world"), "x", pos.get(0), "y", pos.get(1), "z", pos.get(2)), InventoryType.CHEST, 27);
    }

    /** The inventory of a container block ({@code block} descriptor) of the given type. */
    public static Inventory block(final CraftServer server, final Map<String, Object> ref, final InventoryType type, final int size) {
        final HostInventory impl = new HostInventory(server, ref, type, size, null, null);
        final boolean isDouble = Values.list(ref.get("positions")).size() > 1;
        final Class<? extends Inventory> iface = switch (type) {
            case FURNACE, SMOKER, BLAST_FURNACE -> FurnaceInventory.class;
            case CHEST -> isDouble ? DoubleChestInventory.class : Inventory.class;
            default -> Inventory.class;
        };
        return proxy(impl, iface);
    }

    /** {@code Bukkit.createInventory}: a custom inventory whose slots the server keeps. */
    public static Inventory create(final CraftServer server, final @Nullable InventoryHolder holder, final InventoryType type, final int size, final @Nullable Component title) {
        final Component shown = title != null ? title : type.defaultTitle();
        final Map<String, Object> created = Values.map(server.call("inventory.create",
            Values.of("type", type.name(), "size", size, "title", Components.toJson(shown))));
        final long id = Values.i64(created, "id");
        final HostInventory impl = new HostInventory(server, Values.of("kind", "custom", "id", id), type, size, holder, shown);
        final Inventory inventory = proxy(impl, Inventory.class);
        impl.releaseWhenUnreachable(inventory, id);
        CUSTOM.put(id, new WeakReference<>(inventory));
        return inventory;
    }

    /** The inventory a descriptor from the server names (for events and open windows). */
    public static @Nullable Inventory of(final CraftServer server, final Map<String, Object> ref, final InventoryType type, final int size) {
        return switch (Values.str(ref, "kind", "")) {
            case "player" -> {
                final HostPlayer player = server.player(java.util.UUID.fromString(Values.str(ref, "uuid")));
                yield player == null ? null : player.getInventory();
            }
            case "ender" -> {
                final HostPlayer player = server.player(java.util.UUID.fromString(Values.str(ref, "uuid")));
                yield player == null ? null : player.getEnderChest();
            }
            case "crafting" -> {
                final HostPlayer player = server.player(java.util.UUID.fromString(Values.str(ref, "uuid")));
                if (player != null && type == InventoryType.MERCHANT) {
                    // A trading window: payment, payment, result.
                    yield proxy(new HostInventory(server, ref, InventoryType.MERCHANT, 3, null, null), org.bukkit.inventory.MerchantInventory.class);
                }
                yield player == null ? null : crafting(server, player, type == InventoryType.WORKBENCH);
            }
            case "custom" -> {
                final WeakReference<Inventory> known = CUSTOM.get(Values.i64(ref, "id"));
                final Inventory inventory = known == null ? null : known.get();
                // A custom inventory the plugins dropped: a plain view of its slots.
                yield inventory != null ? inventory : proxy(new HostInventory(server, ref, type, size, null, null), Inventory.class);
            }
            case "block" -> block(server, ref, type, size);
            default -> null;
        };
    }

    /** The Bukkit inventory type of a window type name the server reports. */
    public static InventoryType type(final String name) {
        try {
            return InventoryType.valueOf(name.toUpperCase(Locale.ROOT));
        } catch (IllegalArgumentException e) {
            return InventoryType.CHEST;
        }
    }

    /** The view of a window the server describes (see {@code inventory::menu_value}). */
    public static InventoryView view(final CraftServer server, final HostPlayer player, final Map<String, Object> window) {
        final InventoryType type = type(Values.str(window, "type", "CRAFTING"));
        final int size = Values.i32(window, "size", type.getDefaultSize());
        final Inventory top = of(server, Values.map(window.get("top")), type, size);
        final Component title = Components.fromJson(window.get("title"));
        final Class<? extends InventoryView> view = type == InventoryType.MERCHANT ? org.bukkit.inventory.view.MerchantView.class : InventoryView.class;
        return HostProxies.create(new HostInventoryView(server, player, top, type, title), view);
    }

    /** Forget custom inventories (the plugin host is going away). */
    public static void clear() {
        CUSTOM.clear();
    }
}
