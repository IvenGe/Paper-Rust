package io.papermc.paperrs.host.inventory;

import com.google.common.base.Preconditions;
import io.papermc.paperrs.host.HostProxies;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.entity.HostPlayer;
import io.papermc.paperrs.host.item.HostItemStack;
import java.lang.ref.Cleaner;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.ListIterator;
import java.util.Map;
import java.util.NoSuchElementException;
import java.util.Objects;
import java.util.UUID;
import net.kyori.adventure.text.Component;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.block.BlockState;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.HumanEntity;
import org.bukkit.entity.Player;
import org.bukkit.event.inventory.InventoryType;
import org.bukkit.inventory.EquipmentSlot;
import org.bukkit.inventory.Inventory;
import org.bukkit.inventory.InventoryHolder;
import org.bukkit.inventory.ItemStack;
import org.jetbrains.annotations.Nullable;

/**
 * The implementation behind every inventory (Paper's {@code CraftInventory} and its
 * subclasses). The contents live on the Rust server; this object names them with a
 * descriptor ({@code kind}: {@code player}, {@code ender}, {@code block}, {@code custom},
 * {@code crafting}) and reads or writes them over the bridge. Plugins see it through a proxy
 * implementing the right interface ({@code PlayerInventory}, {@code FurnaceInventory}, ...);
 * methods not defined here throw a named {@code UnsupportedOperationException}.
 */
public final class HostInventory implements HostProxies.Backed {
    private static final Cleaner CLEANER = Cleaner.create();

    private final CraftServer server;
    private final Map<String, Object> ref;
    private final InventoryType type;
    private final int size;
    private final @Nullable InventoryHolder holder;
    private final @Nullable Component title;
    private int maxStackSize = 99;
    private Object self;

    HostInventory(final CraftServer server, final Map<String, Object> ref, final InventoryType type, final int size,
                  final @Nullable InventoryHolder holder, final @Nullable Component title) {
        this.server = server;
        this.ref = ref;
        this.type = type;
        this.size = size;
        this.holder = holder;
        this.title = title;
    }

    @Override
    public void bind(final Object self) {
        this.self = self;
    }

    /** Tells the server to forget a custom inventory once this object is gone. */
    void releaseWhenUnreachable(final Object proxy, final long id) {
        final CraftServer server = this.server;
        CLEANER.register(proxy, () -> server.send("inventory.release", Values.of("id", id)));
    }

    public Map<String, Object> ref() {
        return this.ref;
    }

    // --- trading windows (MerchantInventory) ----------------------------------------------

    private Map<String, Object> tradingWindow() {
        return Values.map(this.server.call("merchant.window", Values.of("uuid", Values.str(this.ref, "uuid", ""))));
    }

    public int getSelectedRecipeIndex() {
        return Values.i32(this.tradingWindow(), "selected", 0);
    }

    public org.bukkit.inventory.@Nullable MerchantRecipe getSelectedRecipe() {
        return this.tradingWindow().get("recipe") instanceof Map<?, ?> recipe ? io.papermc.paperrs.host.entity.Merchants.recipe(Values.map(recipe)) : null;
    }

    public org.bukkit.inventory.@Nullable Merchant getMerchant() {
        return io.papermc.paperrs.host.entity.Merchants.merchant(this.server, this.tradingWindow().get("merchant"));
    }

    public String kind() {
        return Values.str(this.ref, "kind", "");
    }

    public @Nullable Component title() {
        return this.title;
    }

    public InventoryType type() {
        return this.type;
    }

    public Inventory self() {
        return (Inventory) this.self;
    }

    // --- contents over the bridge ---------------------------------------------------------------

    /** Every slot, fresh from the server. */
    public HostItemStack[] contents() {
        final List<Object> items = Values.list(Values.map(this.server.call("inventory.contents", Values.of("inventory", this.ref))).get("items"));
        final HostItemStack[] out = new HostItemStack[items.size()];
        for (int i = 0; i < out.length; i++) {
            out[i] = HostItemStack.fromValue(items.get(i));
        }
        return out;
    }

    /** Writes slots (index to stack; {@code null} empties a slot). */
    public void write(final Map<Integer, ? extends @Nullable ItemStack> slots) {
        if (slots.isEmpty()) {
            return;
        }
        final List<Object> changes = new ArrayList<>();
        slots.forEach((index, item) -> changes.add(Arrays.asList((long) index, HostItemStack.toValue(item))));
        this.server.call("inventory.set", Values.of("inventory", this.ref, "slots", changes));
    }

    private void writeOne(final int index, final @Nullable ItemStack item) {
        final Map<Integer, ItemStack> one = new HashMap<>();
        one.put(index, item);
        this.write(one);
    }

    /** A stack that writes changes back to its slot, like CraftBukkit's mirror stacks. */
    private @Nullable ItemStack mirror(final HostItemStack stack, final int index) {
        if (stack.isEmpty()) {
            return this.kind().equals("player") ? ItemStack.empty() : null;
        }
        return stack.mirroring(changed -> this.writeOne(index, changed.clone()));
    }

    // --- Inventory -----------------------------------------------------------------------------

    public int getSize() {
        return this.size;
    }

    public int getMaxStackSize() {
        return this.maxStackSize;
    }

    public void setMaxStackSize(final int size) {
        this.maxStackSize = size;
    }

    public @Nullable ItemStack getItem(final int index) {
        final HostItemStack[] contents = this.contents();
        Preconditions.checkElementIndex(index, contents.length);
        return this.mirror(contents[index], index);
    }

    public void setItem(final int index, final @Nullable ItemStack item) {
        Preconditions.checkElementIndex(index, this.size);
        this.writeOne(index, item == null || item.isEmpty() ? null : HostItemStack.copyOf(item));
    }

    public ItemStack[] getContents() {
        final HostItemStack[] contents = this.contents();
        final ItemStack[] out = new ItemStack[contents.length];
        for (int i = 0; i < out.length; i++) {
            out[i] = contents[i].isEmpty() ? null : contents[i];
        }
        return out;
    }

    public void setContents(final ItemStack[] items) {
        Preconditions.checkArgument(items.length <= this.size, "Invalid inventory size (%s); expected %s or less", items.length, this.size);
        final Map<Integer, ItemStack> slots = new LinkedHashMap<>();
        for (int i = 0; i < this.size; i++) {
            slots.put(i, i < items.length && items[i] != null && !items[i].isEmpty() ? HostItemStack.copyOf(items[i]) : null);
        }
        this.write(slots);
    }

    /** The slots that hold items (not armour, off hand or crafting slots). */
    private int storageSize() {
        return switch (this.kind()) {
            case "player" -> 36;
            case "crafting" -> 0;
            default -> this.size;
        };
    }

    public ItemStack[] getStorageContents() {
        return Arrays.copyOf(this.getContents(), this.storageSize());
    }

    public void setStorageContents(final ItemStack[] items) {
        final int storage = this.storageSize();
        Preconditions.checkArgument(items.length <= storage, "Invalid inventory size (%s); expected %s or less", items.length, storage);
        final Map<Integer, ItemStack> slots = new LinkedHashMap<>();
        for (int i = 0; i < storage; i++) {
            slots.put(i, i < items.length && items[i] != null && !items[i].isEmpty() ? HostItemStack.copyOf(items[i]) : null);
        }
        this.write(slots);
    }

    public boolean contains(final Material material) {
        Preconditions.checkArgument(material != null, "Material cannot be null");
        for (HostItemStack item : this.contents()) {
            if (item.getType() == material && !item.isEmpty()) {
                return true;
            }
        }
        return false;
    }

    public boolean contains(final @Nullable ItemStack item) {
        if (item == null) {
            return false;
        }
        for (HostItemStack stack : this.contents()) {
            if (!stack.isEmpty() && stack.equals(item)) {
                return true;
            }
        }
        return false;
    }

    public boolean contains(final Material material, final int amount) {
        Preconditions.checkArgument(material != null, "Material cannot be null");
        if (amount <= 0) {
            return true;
        }
        int left = amount;
        for (HostItemStack item : this.contents()) {
            if (item.getType() == material && (left -= item.getAmount()) <= 0) {
                return true;
            }
        }
        return false;
    }

    public boolean contains(final @Nullable ItemStack item, final int amount) {
        if (item == null) {
            return false;
        }
        if (amount <= 0) {
            return true;
        }
        int found = 0;
        for (HostItemStack stack : this.contents()) {
            if (!stack.isEmpty() && stack.equals(item) && ++found >= amount) {
                return true;
            }
        }
        return false;
    }

    public boolean containsAtLeast(final @Nullable ItemStack item, final int amount) {
        if (item == null) {
            return false;
        }
        if (amount <= 0) {
            return true;
        }
        int left = amount;
        for (HostItemStack stack : this.contents()) {
            if (!stack.isEmpty() && stack.isSimilar(item) && (left -= stack.getAmount()) <= 0) {
                return true;
            }
        }
        return false;
    }

    public HashMap<Integer, ItemStack> all(final Material material) {
        final HashMap<Integer, ItemStack> out = new HashMap<>();
        final HostItemStack[] contents = this.contents();
        for (int i = 0; i < contents.length; i++) {
            if (contents[i].getType() == material && !contents[i].isEmpty()) {
                out.put(i, contents[i]);
            }
        }
        return out;
    }

    public HashMap<Integer, ItemStack> all(final @Nullable ItemStack item) {
        final HashMap<Integer, ItemStack> out = new HashMap<>();
        if (item == null) {
            return out;
        }
        final HostItemStack[] contents = this.contents();
        for (int i = 0; i < contents.length; i++) {
            if (!contents[i].isEmpty() && contents[i].equals(item)) {
                out.put(i, contents[i]);
            }
        }
        return out;
    }

    public int first(final Material material) {
        final HostItemStack[] contents = this.contents();
        for (int i = 0; i < contents.length; i++) {
            if (contents[i].getType() == material && !contents[i].isEmpty()) {
                return i;
            }
        }
        return -1;
    }

    public int first(final ItemStack item) {
        return this.first(this.contents(), item, true);
    }

    private int first(final HostItemStack[] contents, final @Nullable ItemStack item, final boolean withAmount) {
        if (item == null) {
            return -1;
        }
        for (int i = 0; i < this.storageSize() && i < contents.length; i++) {
            if (contents[i].isEmpty()) {
                continue;
            }
            if (withAmount ? contents[i].equals(item) : contents[i].isSimilar(item)) {
                return i;
            }
        }
        return -1;
    }

    public int firstEmpty() {
        final HostItemStack[] contents = this.contents();
        for (int i = 0; i < this.storageSize() && i < contents.length; i++) {
            if (contents[i].isEmpty()) {
                return i;
            }
        }
        return -1;
    }

    public boolean isEmpty() {
        for (HostItemStack item : this.contents()) {
            if (!item.isEmpty()) {
                return false;
            }
        }
        return true;
    }

    /** CraftInventory's addItem: top up similar stacks first, then fill empty slots. */
    public HashMap<Integer, ItemStack> addItem(final ItemStack... items) {
        Preconditions.checkArgument(items != null, "items cannot be null");
        final HashMap<Integer, ItemStack> leftover = new HashMap<>();
        final HostItemStack[] contents = this.contents();
        final Map<Integer, ItemStack> changed = new LinkedHashMap<>();
        final int storage = Math.min(this.storageSize(), contents.length);
        for (int i = 0; i < items.length; i++) {
            final ItemStack item = items[i];
            Preconditions.checkArgument(item != null, "ItemStack cannot be null");
            if (item.isEmpty()) {
                continue;
            }
            final HostItemStack adding = HostItemStack.copyOf(item);
            int left = adding.getAmount();
            final int max = Math.min(adding.getMaxStackSize(), this.maxStackSize);
            for (int slot = 0; slot < storage && left > 0; slot++) {
                final HostItemStack existing = contents[slot];
                if (!existing.isEmpty() && existing.isSimilar(adding) && existing.getAmount() < max) {
                    final int moved = Math.min(max - existing.getAmount(), left);
                    existing.setAmount(existing.getAmount() + moved);
                    left -= moved;
                    changed.put(slot, existing);
                }
            }
            for (int slot = 0; slot < storage && left > 0; slot++) {
                if (contents[slot].isEmpty()) {
                    final int moved = Math.min(max, left);
                    final HostItemStack placed = adding.clone();
                    placed.setAmount(moved);
                    contents[slot] = placed;
                    left -= moved;
                    changed.put(slot, placed);
                }
            }
            if (left > 0) {
                final HostItemStack rest = adding.clone();
                rest.setAmount(left);
                leftover.put(i, rest);
            }
        }
        this.write(changed);
        return leftover;
    }

    public HashMap<Integer, ItemStack> removeItem(final ItemStack... items) {
        return this.remove(items, false);
    }

    public HashMap<Integer, ItemStack> removeItemAnySlot(final ItemStack... items) {
        return this.remove(items, true);
    }

    private HashMap<Integer, ItemStack> remove(final ItemStack[] items, final boolean anySlot) {
        Preconditions.checkArgument(items != null, "items cannot be null");
        final HashMap<Integer, ItemStack> leftover = new HashMap<>();
        final HostItemStack[] contents = this.contents();
        final Map<Integer, ItemStack> changed = new LinkedHashMap<>();
        final int limit = anySlot ? contents.length : Math.min(this.storageSize(), contents.length);
        for (int i = 0; i < items.length; i++) {
            final ItemStack item = items[i];
            Preconditions.checkArgument(item != null, "ItemStack cannot be null");
            int left = item.getAmount();
            for (int slot = 0; slot < limit && left > 0; slot++) {
                final HostItemStack existing = contents[slot];
                if (existing.isEmpty() || !existing.isSimilar(item)) {
                    continue;
                }
                final int taken = Math.min(existing.getAmount(), left);
                left -= taken;
                if (taken == existing.getAmount()) {
                    contents[slot] = HostItemStack.empty();
                    changed.put(slot, null);
                } else {
                    existing.setAmount(existing.getAmount() - taken);
                    changed.put(slot, existing);
                }
            }
            if (left > 0) {
                final HostItemStack rest = HostItemStack.copyOf(item);
                rest.setAmount(left);
                leftover.put(i, rest);
            }
        }
        this.write(changed);
        return leftover;
    }

    public void remove(final Material material) {
        final HostItemStack[] contents = this.contents();
        final Map<Integer, ItemStack> changed = new LinkedHashMap<>();
        for (int i = 0; i < contents.length; i++) {
            if (contents[i].getType() == material && !contents[i].isEmpty()) {
                changed.put(i, null);
            }
        }
        this.write(changed);
    }

    public void remove(final ItemStack item) {
        final HostItemStack[] contents = this.contents();
        final Map<Integer, ItemStack> changed = new LinkedHashMap<>();
        for (int i = 0; i < contents.length; i++) {
            if (!contents[i].isEmpty() && contents[i].equals(item)) {
                changed.put(i, null);
            }
        }
        this.write(changed);
    }

    public void clear(final int index) {
        this.setItem(index, null);
    }

    public void clear() {
        final Map<Integer, ItemStack> slots = new LinkedHashMap<>();
        for (int i = 0; i < this.size; i++) {
            slots.put(i, null);
        }
        this.write(slots);
    }

    public int close() {
        final List<HumanEntity> viewers = this.getViewers();
        for (HumanEntity viewer : viewers) {
            viewer.closeInventory();
        }
        return viewers.size();
    }

    public List<HumanEntity> getViewers() {
        final List<HumanEntity> out = new ArrayList<>();
        for (Object uuid : Values.list(this.server.call("inventory.viewers", Values.of("inventory", this.ref)))) {
            final HostPlayer player = this.server.player(UUID.fromString(String.valueOf(uuid)));
            if (player != null) {
                out.add(player);
            }
        }
        return out;
    }

    public InventoryType getType() {
        return this.type;
    }

    public @Nullable InventoryHolder getHolder() {
        return this.getHolder(true);
    }

    public @Nullable InventoryHolder getHolder(final boolean useSnapshot) {
        if (this.holder != null) {
            return this.holder;
        }
        return switch (this.kind()) {
            case "player", "crafting" -> this.server.player(UUID.fromString(Values.str(this.ref, "uuid")));
            case "block" -> {
                if (this.self instanceof org.bukkit.inventory.DoubleChestInventory doubleChest) {
                    yield new org.bukkit.block.DoubleChest(doubleChest);
                }
                final BlockState state = Inventories.blockOf(this.server, this.ref).getState(useSnapshot);
                yield state instanceof InventoryHolder h ? h : null;
            }
            default -> null;
        };
    }

    public @Nullable Location getLocation() {
        return switch (this.kind()) {
            case "block" -> Inventories.blockOf(this.server, this.ref).getLocation();
            case "player", "ender", "crafting" -> {
                final Player player = this.server.player(UUID.fromString(Values.str(this.ref, "uuid")));
                yield player == null ? null : player.getLocation();
            }
            default -> null;
        };
    }

    public ListIterator<ItemStack> iterator() {
        return this.iterator(0);
    }

    public ListIterator<ItemStack> iterator(final int index) {
        final ItemStack[] contents = this.getContents();
        final int start = index < 0 ? contents.length + index : index;
        return new ListIterator<>() {
            private int next = start;
            private int last = -1;

            @Override
            public boolean hasNext() {
                return this.next < contents.length;
            }

            @Override
            public ItemStack next() {
                if (!this.hasNext()) {
                    throw new NoSuchElementException();
                }
                this.last = this.next++;
                return contents[this.last];
            }

            @Override
            public boolean hasPrevious() {
                return this.next > 0;
            }

            @Override
            public ItemStack previous() {
                if (!this.hasPrevious()) {
                    throw new NoSuchElementException();
                }
                this.last = --this.next;
                return contents[this.last];
            }

            @Override
            public int nextIndex() {
                return this.next;
            }

            @Override
            public int previousIndex() {
                return this.next - 1;
            }

            @Override
            public void remove() {
                throw new UnsupportedOperationException("Can't change the size of an inventory!");
            }

            @Override
            public void set(final ItemStack item) {
                Preconditions.checkState(this.last >= 0, "No current item!");
                HostInventory.this.setItem(this.last, item);
            }

            @Override
            public void add(final ItemStack item) {
                throw new UnsupportedOperationException("Can't change the size of an inventory!");
            }
        };
    }

    // --- PlayerInventory (Bukkit slots: 0-8 hotbar, 9-35 main, 36-39 boots..helmet, 40 off hand) --

    private HostPlayer owner() {
        return Objects.requireNonNull(this.server.player(UUID.fromString(Values.str(this.ref, "uuid"))), "the player is offline");
    }

    private static int slotOf(final EquipmentSlot slot, final int held) {
        return switch (slot) {
            case HAND -> held;
            case OFF_HAND -> 40;
            case FEET -> 36;
            case LEGS -> 37;
            case CHEST -> 38;
            case HEAD -> 39;
            default -> throw new IllegalArgumentException("Not implemented. This is a bug");
        };
    }

    public ItemStack[] getArmorContents() {
        return Arrays.copyOfRange(this.getContents(), 36, 40);
    }

    public ItemStack[] getExtraContents() {
        return Arrays.copyOfRange(this.getContents(), 40, 41);
    }

    public void setArmorContents(final ItemStack[] items) {
        Preconditions.checkArgument(items.length <= 4, "Invalid inventory size (%s); expected 4 or less", items.length);
        final Map<Integer, ItemStack> slots = new LinkedHashMap<>();
        for (int i = 0; i < 4; i++) {
            slots.put(36 + i, i < items.length && items[i] != null && !items[i].isEmpty() ? HostItemStack.copyOf(items[i]) : null);
        }
        this.write(slots);
    }

    public void setExtraContents(final ItemStack[] items) {
        Preconditions.checkArgument(items.length <= 1, "Invalid inventory size (%s); expected 1 or less", items.length);
        this.setItem(40, items.length == 0 ? null : items[0]);
    }

    public @Nullable ItemStack getHelmet() {
        return this.getItem(39);
    }

    public @Nullable ItemStack getChestplate() {
        return this.getItem(38);
    }

    public @Nullable ItemStack getLeggings() {
        return this.getItem(37);
    }

    public @Nullable ItemStack getBoots() {
        return this.getItem(36);
    }

    public void setHelmet(final @Nullable ItemStack item) {
        this.setItem(39, item);
    }

    public void setChestplate(final @Nullable ItemStack item) {
        this.setItem(38, item);
    }

    public void setLeggings(final @Nullable ItemStack item) {
        this.setItem(37, item);
    }

    public void setBoots(final @Nullable ItemStack item) {
        this.setItem(36, item);
    }

    public void setItem(final EquipmentSlot slot, final @Nullable ItemStack item) {
        this.setItem(slotOf(slot, this.getHeldItemSlot()), item);
    }

    public ItemStack getItem(final EquipmentSlot slot) {
        final ItemStack item = this.getItem(slotOf(slot, this.getHeldItemSlot()));
        return item == null ? ItemStack.empty() : item;
    }

    public ItemStack getItemInMainHand() {
        return this.getItem(EquipmentSlot.HAND);
    }

    public void setItemInMainHand(final @Nullable ItemStack item) {
        this.setItem(EquipmentSlot.HAND, item);
    }

    public ItemStack getItemInOffHand() {
        return this.getItem(EquipmentSlot.OFF_HAND);
    }

    public void setItemInOffHand(final @Nullable ItemStack item) {
        this.setItem(EquipmentSlot.OFF_HAND, item);
    }

    public ItemStack getItemInHand() {
        return this.getItemInMainHand();
    }

    public void setItemInHand(final @Nullable ItemStack item) {
        this.setItemInMainHand(item);
    }

    public int getHeldItemSlot() {
        return this.owner().getHeldSlot();
    }

    public void setHeldItemSlot(final int slot) {
        Preconditions.checkArgument(slot >= 0 && slot < 9, "Slot (%s) is not between 0 and 8 inclusive", slot);
        this.owner().setHeldSlot(slot);
    }

    // --- FurnaceInventory (0 smelting, 1 fuel, 2 result) and CraftingInventory (0 result) -------

    public @Nullable ItemStack getResult() {
        return this.kind().equals("crafting") ? this.getItem(0) : this.getItem(2);
    }

    public @Nullable ItemStack getFuel() {
        return this.getItem(1);
    }

    public @Nullable ItemStack getSmelting() {
        return this.getItem(0);
    }

    public void setResult(final @Nullable ItemStack item) {
        this.setItem(this.kind().equals("crafting") ? 0 : 2, item);
    }

    public void setFuel(final @Nullable ItemStack item) {
        this.setItem(1, item);
    }

    public void setSmelting(final @Nullable ItemStack item) {
        this.setItem(0, item);
    }

    public ItemStack[] getMatrix() {
        final ItemStack[] contents = this.getContents();
        return Arrays.copyOfRange(contents, 1, contents.length);
    }

    public void setMatrix(final ItemStack[] items) {
        Preconditions.checkArgument(items.length <= this.size - 1, "Invalid inventory size (%s); expected %s or less", items.length, this.size - 1);
        final Map<Integer, ItemStack> slots = new LinkedHashMap<>();
        for (int i = 0; i < this.size - 1; i++) {
            slots.put(i + 1, i < items.length && items[i] != null && !items[i].isEmpty() ? HostItemStack.copyOf(items[i]) : null);
        }
        this.write(slots);
    }

    // --- DoubleChestInventory -------------------------------------------------------------------

    public Inventory getLeftSide() {
        return Inventories.half(this.server, this.ref, 0);
    }

    public Inventory getRightSide() {
        return Inventories.half(this.server, this.ref, 1);
    }

    // --- object -------------------------------------------------------------------------------

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostInventory other && this.ref.equals(other.ref);
    }

    @Override
    public int hashCode() {
        return this.ref.hashCode();
    }

    @Override
    public String toString() {
        return "CraftInventory{" + this.type + ", " + this.ref + "}";
    }
}
