package io.papermc.paperrs.host.event;

import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.entity.Entities;
import io.papermc.paperrs.host.entity.HostPlayer;
import io.papermc.paperrs.host.inventory.Inventories;
import io.papermc.paperrs.host.item.HostItemStack;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.bukkit.GameMode;
import org.bukkit.NamespacedKey;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.Entity;
import org.bukkit.entity.Item;
import org.bukkit.event.Event;
import org.bukkit.event.entity.EntityPickupItemEvent;
import org.bukkit.event.inventory.ClickType;
import org.bukkit.event.inventory.CraftItemEvent;
import org.bukkit.event.inventory.InventoryAction;
import org.bukkit.event.inventory.InventoryClickEvent;
import org.bukkit.event.inventory.InventoryCloseEvent;
import org.bukkit.event.inventory.InventoryDragEvent;
import org.bukkit.event.inventory.InventoryOpenEvent;
import org.bukkit.event.inventory.InventoryType;
import org.bukkit.event.player.PlayerAttemptPickupItemEvent;
import org.bukkit.event.player.PlayerDropItemEvent;
import org.bukkit.event.player.PlayerPickupItemEvent;
import org.bukkit.inventory.InventoryView;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.ShapelessRecipe;

/** The inventory and item events the server fires (see {@code plugins::events}). */
final class InventoryEvents {
    private final CraftServer server;

    InventoryEvents(final CraftServer server) {
        this.server = server;
    }

    private HostPlayer player(final Map<String, Object> args) {
        final HostPlayer player = this.server.player(Values.uuid(args, "uuid"));
        if (player == null) {
            throw new IllegalArgumentException("player " + args.get("uuid") + " is not online");
        }
        return player;
    }

    Object open(final Map<String, Object> args) {
        final HostPlayer player = this.player(args);
        final InventoryView view = Inventories.view(this.server, player, Values.map(args.get("inventory")));
        final InventoryOpenEvent event = new InventoryOpenEvent(view);
        event.callEvent();
        return Values.of("cancelled", event.isCancelled());
    }

    Object close(final Map<String, Object> args) {
        final HostPlayer player = this.player(args);
        final InventoryView view = Inventories.view(this.server, player, Values.map(args.get("inventory")));
        InventoryCloseEvent.Reason reason;
        try {
            reason = InventoryCloseEvent.Reason.valueOf(Values.str(args, "reason", "UNKNOWN"));
        } catch (IllegalArgumentException e) {
            reason = InventoryCloseEvent.Reason.UNKNOWN;
        }
        try {
            new InventoryCloseEvent(view, reason).callEvent();
        } finally {
            player.windowClosed();
        }
        return Values.of();
    }

    /** InventoryClickEvent, CraftItemEvent or InventoryDragEvent for a click the server got. */
    Object click(final Map<String, Object> args) {
        final HostPlayer player = this.player(args);
        final Map<String, Object> window = Values.map(args.get("inventory"));
        final InventoryView view = Inventories.view(this.server, player, window);
        final int slot = Values.i32(args, "slot", -999);
        final int button = Values.i32(args, "button", 0);
        final int mode = Values.i32(args, "mode", 0);
        final HostItemStack cursor = HostItemStack.fromValue(args.get("cursor"));
        if (mode == 5) {
            return this.drag(args, view, cursor);
        }
        final ItemStack current = slot >= 0 ? view.getItem(slot) : null;
        final ClickType click = clickType(mode, button, slot);
        final InventoryAction action = action(mode, button, slot, current, cursor, view, player);
        final InventoryType.SlotType slotType = slot == -999 ? InventoryType.SlotType.OUTSIDE : view.getSlotType(slot);
        final int hotbar = mode == 2 && button >= 0 && button < 9 ? button : -1;
        final InventoryClickEvent event;
        if (args.get("craft") instanceof Map<?, ?>) {
            final HostItemStack result = HostItemStack.fromValue(args.get("craft"));
            // The recipe as Paper-RS knows it: its result (ingredients are not tracked here).
            final ShapelessRecipe recipe = new ShapelessRecipe(NamespacedKey.minecraft(result.getType().getKey().getKey()), result);
            event = new CraftItemEvent(recipe, view, slotType, slot, click, action, hotbar);
        } else {
            event = new InventoryClickEvent(view, slotType, slot, click, action, hotbar);
        }
        event.callEvent();
        return Values.of("cancelled", event.getResult() == Event.Result.DENY);
    }

    private Object drag(final Map<String, Object> args, final InventoryView view, final HostItemStack cursor) {
        final List<Object> slots = Values.list(args.get("drag_slots"));
        final int kind = Values.i32(args, "drag_kind", 0);
        final Map<Integer, ItemStack> placed = new LinkedHashMap<>();
        int left = cursor.getAmount();
        if (!cursor.isEmpty() && !slots.isEmpty()) {
            final int per = switch (kind) {
                case 0 -> cursor.getAmount() / slots.size();
                case 1 -> 1;
                default -> cursor.getMaxStackSize();
            };
            for (Object s : slots) {
                final int raw = ((Number) s).intValue();
                final ItemStack existing = view.getItem(raw);
                final int had = existing == null || existing.isEmpty() ? 0 : existing.getAmount();
                int add = Math.max(0, Math.min(per, cursor.getMaxStackSize() - had));
                if (kind != 2) {
                    add = Math.min(add, left);
                    left -= add;
                }
                if (add > 0) {
                    final HostItemStack stack = cursor.clone();
                    stack.setAmount(had + add);
                    placed.put(raw, stack);
                }
            }
        }
        final ItemStack newCursor;
        if (kind == 2) {
            newCursor = cursor.clone();
        } else if (left > 0) {
            final HostItemStack rest = cursor.clone();
            rest.setAmount(left);
            newCursor = rest;
        } else {
            newCursor = null;
        }
        final InventoryDragEvent event = new InventoryDragEvent(view, newCursor, cursor.clone(), kind == 1, placed);
        event.callEvent();
        return Values.of("cancelled", event.getResult() == Event.Result.DENY);
    }

    static ClickType clickType(final int mode, final int button, final int slot) {
        return switch (mode) {
            case 0 -> slot == -1 ? (button == 0 ? ClickType.WINDOW_BORDER_LEFT : ClickType.WINDOW_BORDER_RIGHT) : button == 0 ? ClickType.LEFT : ClickType.RIGHT;
            case 1 -> button == 0 ? ClickType.SHIFT_LEFT : ClickType.SHIFT_RIGHT;
            case 2 -> button == 40 ? ClickType.SWAP_OFFHAND : ClickType.NUMBER_KEY;
            case 3 -> ClickType.MIDDLE;
            case 4 -> slot == -999 ? (button == 0 ? ClickType.WINDOW_BORDER_LEFT : ClickType.WINDOW_BORDER_RIGHT) : button == 0 ? ClickType.DROP : ClickType.CONTROL_DROP;
            case 6 -> ClickType.DOUBLE_CLICK;
            default -> ClickType.UNKNOWN;
        };
    }

    private static InventoryAction action(final int mode, final int button, final int slot, final ItemStack current, final ItemStack cursor,
                                          final InventoryView view, final HostPlayer player) {
        final boolean hasCurrent = current != null && !current.isEmpty();
        final boolean hasCursor = cursor != null && !cursor.isEmpty();
        return switch (mode) {
            case 0 -> {
                if (slot == -999) {
                    yield hasCursor ? (button == 0 ? InventoryAction.DROP_ALL_CURSOR : InventoryAction.DROP_ONE_CURSOR) : InventoryAction.NOTHING;
                }
                if (slot < 0) {
                    yield InventoryAction.NOTHING;
                }
                if (view.getSlotType(slot) == InventoryType.SlotType.RESULT) {
                    yield hasCurrent ? InventoryAction.PICKUP_ALL : InventoryAction.NOTHING;
                }
                if (!hasCurrent && !hasCursor) {
                    yield InventoryAction.NOTHING;
                }
                if (!hasCursor) {
                    yield button == 0 ? InventoryAction.PICKUP_ALL : InventoryAction.PICKUP_HALF;
                }
                if (!hasCurrent) {
                    yield button == 0 ? InventoryAction.PLACE_ALL : InventoryAction.PLACE_ONE;
                }
                if (current.isSimilar(cursor)) {
                    final int space = current.getMaxStackSize() - current.getAmount();
                    if (space <= 0) {
                        yield InventoryAction.NOTHING;
                    }
                    yield button == 1 ? InventoryAction.PLACE_ONE : cursor.getAmount() <= space ? InventoryAction.PLACE_ALL : InventoryAction.PLACE_SOME;
                }
                yield InventoryAction.SWAP_WITH_CURSOR;
            }
            case 1 -> hasCurrent ? InventoryAction.MOVE_TO_OTHER_INVENTORY : InventoryAction.NOTHING;
            case 2 -> hasCurrent || button == 40 ? InventoryAction.HOTBAR_SWAP : InventoryAction.NOTHING;
            case 3 -> hasCurrent && player.getGameMode() == GameMode.CREATIVE && !hasCursor ? InventoryAction.CLONE_STACK : InventoryAction.NOTHING;
            case 4 -> hasCurrent ? (button == 0 ? InventoryAction.DROP_ONE_SLOT : InventoryAction.DROP_ALL_SLOT) : InventoryAction.NOTHING;
            case 6 -> hasCursor ? InventoryAction.COLLECT_TO_CURSOR : InventoryAction.NOTHING;
            default -> InventoryAction.UNKNOWN;
        };
    }

    /** PlayerAttemptPickupItemEvent, then EntityPickupItemEvent (and the old PlayerPickupItemEvent). */
    Object pickup(final Map<String, Object> args) {
        final HostPlayer player = this.player(args);
        final Entity entity = Entities.byId(this.server, Values.i32(args, "item_entity"));
        if (!(entity instanceof Item item)) {
            return Values.of("cancelled", false);
        }
        final int remaining = Values.i32(args, "remaining", 0);
        final PlayerAttemptPickupItemEvent attempt = new PlayerAttemptPickupItemEvent(player, item, remaining);
        attempt.callEvent();
        if (attempt.isCancelled()) {
            return Values.of("cancelled", true);
        }
        final PlayerPickupItemEvent legacy = new PlayerPickupItemEvent(player, item, remaining);
        legacy.callEvent();
        final EntityPickupItemEvent event = new EntityPickupItemEvent(player, item, remaining);
        event.setCancelled(legacy.isCancelled());
        event.callEvent();
        return Values.of("cancelled", event.isCancelled());
    }

    Object drop(final Map<String, Object> args) {
        final HostPlayer player = this.player(args);
        final Entity entity = Entities.byId(this.server, Values.i32(args, "item_entity"));
        if (!(entity instanceof Item item)) {
            return Values.of("cancelled", false);
        }
        final PlayerDropItemEvent event = new PlayerDropItemEvent(player, item);
        event.callEvent();
        return Values.of("cancelled", event.isCancelled());
    }
}
