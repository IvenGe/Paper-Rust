package io.papermc.paperrs.host.entity;

import io.papermc.paperrs.host.HostProxies;
import org.bukkit.entity.Entity;
import org.bukkit.inventory.EntityEquipment;
import org.bukkit.inventory.EquipmentSlot;
import org.bukkit.inventory.ItemStack;
import org.jetbrains.annotations.Nullable;

/**
 * {@code LivingEntity#getEquipment()}: a player's armour and hands (their inventory), or a
 * mob's equipment. Drop chances are not modelled yet and fail with a named
 * {@code UnsupportedOperationException}.
 */
public final class HostEquipment {
    private final @Nullable HostPlayer player;
    private final @Nullable HostEntity mob;

    private HostEquipment(final @Nullable HostPlayer player, final @Nullable HostEntity mob) {
        this.player = player;
        this.mob = mob;
    }

    public static EntityEquipment of(final HostPlayer player) {
        return HostProxies.create(new HostEquipment(player, null), EntityEquipment.class);
    }

    public static EntityEquipment of(final HostEntity mob) {
        return HostProxies.create(new HostEquipment(null, mob), EntityEquipment.class);
    }

    /** Mob equipment index: main hand, off hand, feet, legs, chest, head. */
    private static int mobSlot(final EquipmentSlot slot) {
        return switch (slot) {
            case HAND -> 0;
            case OFF_HAND -> 1;
            case FEET -> 2;
            case LEGS -> 3;
            case CHEST -> 4;
            case HEAD -> 5;
            default -> throw new IllegalArgumentException(slot + " is not supported for this entity");
        };
    }

    public ItemStack getItem(final EquipmentSlot slot) {
        if (this.player != null) {
            return this.player.getInventory().getItem(slot);
        }
        return this.mob.equipment(mobSlot(slot));
    }

    public void setItem(final EquipmentSlot slot, final @Nullable ItemStack item) {
        if (this.player != null) {
            this.player.getInventory().setItem(slot, item);
        } else {
            this.mob.setEquipment(mobSlot(slot), item);
        }
    }

    public void setItem(final EquipmentSlot slot, final @Nullable ItemStack item, final boolean silent) {
        this.setItem(slot, item);
    }

    public ItemStack getItemInMainHand() {
        return this.getItem(EquipmentSlot.HAND);
    }

    public void setItemInMainHand(final @Nullable ItemStack item) {
        this.setItem(EquipmentSlot.HAND, item);
    }

    public void setItemInMainHand(final @Nullable ItemStack item, final boolean silent) {
        this.setItem(EquipmentSlot.HAND, item);
    }

    public ItemStack getItemInOffHand() {
        return this.getItem(EquipmentSlot.OFF_HAND);
    }

    public void setItemInOffHand(final @Nullable ItemStack item) {
        this.setItem(EquipmentSlot.OFF_HAND, item);
    }

    public void setItemInOffHand(final @Nullable ItemStack item, final boolean silent) {
        this.setItem(EquipmentSlot.OFF_HAND, item);
    }

    public ItemStack getItemInHand() {
        return this.getItemInMainHand();
    }

    public void setItemInHand(final @Nullable ItemStack item) {
        this.setItemInMainHand(item);
    }

    public ItemStack getHelmet() {
        return this.getItem(EquipmentSlot.HEAD);
    }

    public void setHelmet(final @Nullable ItemStack item) {
        this.setItem(EquipmentSlot.HEAD, item);
    }

    public void setHelmet(final @Nullable ItemStack item, final boolean silent) {
        this.setItem(EquipmentSlot.HEAD, item);
    }

    public ItemStack getChestplate() {
        return this.getItem(EquipmentSlot.CHEST);
    }

    public void setChestplate(final @Nullable ItemStack item) {
        this.setItem(EquipmentSlot.CHEST, item);
    }

    public void setChestplate(final @Nullable ItemStack item, final boolean silent) {
        this.setItem(EquipmentSlot.CHEST, item);
    }

    public ItemStack getLeggings() {
        return this.getItem(EquipmentSlot.LEGS);
    }

    public void setLeggings(final @Nullable ItemStack item) {
        this.setItem(EquipmentSlot.LEGS, item);
    }

    public void setLeggings(final @Nullable ItemStack item, final boolean silent) {
        this.setItem(EquipmentSlot.LEGS, item);
    }

    public ItemStack getBoots() {
        return this.getItem(EquipmentSlot.FEET);
    }

    public void setBoots(final @Nullable ItemStack item) {
        this.setItem(EquipmentSlot.FEET, item);
    }

    public void setBoots(final @Nullable ItemStack item, final boolean silent) {
        this.setItem(EquipmentSlot.FEET, item);
    }

    public ItemStack[] getArmorContents() {
        return new ItemStack[] {this.getBoots(), this.getLeggings(), this.getChestplate(), this.getHelmet()};
    }

    public void setArmorContents(final ItemStack[] items) {
        final EquipmentSlot[] slots = {EquipmentSlot.FEET, EquipmentSlot.LEGS, EquipmentSlot.CHEST, EquipmentSlot.HEAD};
        for (int i = 0; i < slots.length; i++) {
            this.setItem(slots[i], i < items.length ? items[i] : null);
        }
    }

    public void clear() {
        for (EquipmentSlot slot : new EquipmentSlot[] {EquipmentSlot.HAND, EquipmentSlot.OFF_HAND, EquipmentSlot.FEET, EquipmentSlot.LEGS, EquipmentSlot.CHEST, EquipmentSlot.HEAD}) {
            this.setItem(slot, null);
        }
    }

    public Entity getHolder() {
        return this.player != null ? this.player : this.mob.self();
    }
}
