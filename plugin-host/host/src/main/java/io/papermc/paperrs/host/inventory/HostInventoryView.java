package io.papermc.paperrs.host.inventory;

import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.entity.HostPlayer;
import java.util.Objects;
import net.kyori.adventure.text.Component;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.HumanEntity;
import org.bukkit.event.inventory.InventoryType;
import org.bukkit.inventory.Inventory;
import org.bukkit.inventory.InventoryView;
import org.bukkit.inventory.ItemStack;
import org.jetbrains.annotations.Nullable;

/**
 * A window a player has open (Paper's {@code CraftInventoryView}): the top inventory (a
 * container, or the 2x2 crafting grid of the player's own inventory) above the player's
 * inventory. Raw slots number the top inventory first, then the player's main inventory and
 * hotbar, as the client does.
 */
public final class HostInventoryView {
    private final CraftServer server;
    private final HostPlayer player;
    private final @Nullable Inventory top;
    private final InventoryType type;
    private final @Nullable Component title;

    HostInventoryView(final CraftServer server, final HostPlayer player, final @Nullable Inventory top, final InventoryType type, final @Nullable Component title) {
        this.server = server;
        this.player = player;
        this.top = top;
        this.type = type;
        this.title = title;
    }

    private boolean ownInventory() {
        return this.type == InventoryType.CRAFTING;
    }

    public Inventory getTopInventory() {
        return Objects.requireNonNull(this.top, "the window has no top inventory");
    }

    public Inventory getBottomInventory() {
        return this.player.getInventory();
    }

    public HumanEntity getPlayer() {
        return this.player;
    }

    public InventoryType getType() {
        return this.type;
    }

    private int topSize() {
        return this.top == null ? 0 : this.top.getSize();
    }

    public int countSlots() {
        // The player inventory window also shows armour and the off hand.
        return this.ownInventory() ? 46 : this.topSize() + 36;
    }

    /** The slot a raw slot is within the inventory {@link #getInventory(int)} returns. */
    public int convertSlot(final int rawSlot) {
        if (this.ownInventory()) {
            if (rawSlot < 5) {
                return rawSlot;
            }
            if (rawSlot < 9) {
                return 39 - (rawSlot - 5);
            }
            if (rawSlot < 36) {
                return rawSlot;
            }
            if (rawSlot < 45) {
                return rawSlot - 36;
            }
            return 40;
        }
        final int size = this.topSize();
        if (rawSlot < size) {
            return rawSlot;
        }
        final int bottom = rawSlot - size;
        // The bottom part shows the main inventory (Bukkit 9-35) then the hotbar (0-8).
        return bottom < 27 ? bottom + 9 : bottom - 27;
    }

    public @Nullable Inventory getInventory(final int rawSlot) {
        if (rawSlot == InventoryView.OUTSIDE || rawSlot == -1) {
            return null;
        }
        if (this.ownInventory()) {
            return rawSlot < 5 ? this.top : this.getBottomInventory();
        }
        return rawSlot < this.topSize() ? this.top : this.getBottomInventory();
    }

    public InventoryType.SlotType getSlotType(final int rawSlot) {
        if (rawSlot < 0) {
            return InventoryType.SlotType.OUTSIDE;
        }
        if (this.ownInventory()) {
            if (rawSlot == 0) {
                return InventoryType.SlotType.RESULT;
            }
            if (rawSlot < 5) {
                return InventoryType.SlotType.CRAFTING;
            }
            if (rawSlot < 9) {
                return InventoryType.SlotType.ARMOR;
            }
            return rawSlot >= 36 && rawSlot < 45 ? InventoryType.SlotType.QUICKBAR : InventoryType.SlotType.CONTAINER;
        }
        final int size = this.topSize();
        if (rawSlot < size) {
            return switch (this.type) {
                case WORKBENCH -> rawSlot == 0 ? InventoryType.SlotType.RESULT : InventoryType.SlotType.CRAFTING;
                case FURNACE, SMOKER, BLAST_FURNACE -> rawSlot == 2 ? InventoryType.SlotType.RESULT : rawSlot == 1 ? InventoryType.SlotType.FUEL : InventoryType.SlotType.CRAFTING;
                default -> InventoryType.SlotType.CONTAINER;
            };
        }
        return rawSlot - size >= 27 ? InventoryType.SlotType.QUICKBAR : InventoryType.SlotType.CONTAINER;
    }

    public @Nullable ItemStack getItem(final int rawSlot) {
        final Inventory inventory = this.getInventory(rawSlot);
        return inventory == null ? null : inventory.getItem(this.convertSlot(rawSlot));
    }

    public void setItem(final int rawSlot, final @Nullable ItemStack item) {
        final Inventory inventory = this.getInventory(rawSlot);
        if (inventory != null) {
            inventory.setItem(this.convertSlot(rawSlot), item);
        } else if (item != null && !item.isEmpty()) {
            this.player.getWorld().dropItemNaturally(this.player.getLocation(), item);
        }
    }

    public ItemStack getCursor() {
        return this.player.getItemOnCursor();
    }

    public void setCursor(final @Nullable ItemStack item) {
        this.player.setItemOnCursor(item);
    }

    public void close() {
        this.player.closeInventory();
    }

    public void open() {
        if (this.top != null && !this.ownInventory()) {
            this.player.openInventory(this.top);
        }
    }

    public Component title() {
        return this.title != null ? this.title : this.type.defaultTitle();
    }

    public String getTitle() {
        return Components.toLegacy(this.title());
    }

    public String getOriginalTitle() {
        return this.getTitle();
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostInventoryView v && v.player.equals(this.player) && Objects.equals(v.top, this.top) && v.type == this.type;
    }

    @Override
    public int hashCode() {
        return Objects.hash(this.player, this.top, this.type);
    }

    @Override
    public String toString() {
        return "CraftInventoryView{" + this.type + ", " + this.top + "}";
    }

    @SuppressWarnings("unused")
    private CraftServer server() {
        return this.server;
    }
}
