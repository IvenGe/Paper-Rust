package io.papermc.paperrs.host.entity;

import io.papermc.paper.entity.TeleportFlag;
import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.HostProxies;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.item.HostItemStack;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import net.kyori.adventure.text.Component;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.Server;
import org.bukkit.World;
import org.bukkit.block.BlockFace;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.AbstractArrow;
import org.bukkit.entity.Entity;
import org.bukkit.entity.EntityType;
import org.bukkit.entity.LivingEntity;
import org.bukkit.event.player.PlayerTeleportEvent;
import org.bukkit.inventory.EntityEquipment;
import org.bukkit.inventory.ItemStack;
import org.bukkit.metadata.MetadataValue;
import org.bukkit.permissions.PermissibleBase;
import org.bukkit.permissions.Permission;
import org.bukkit.permissions.PermissionAttachment;
import org.bukkit.permissions.PermissionAttachmentInfo;
import org.bukkit.persistence.PersistentDataContainer;
import org.bukkit.plugin.Plugin;
import org.bukkit.projectiles.ProjectileSource;
import org.bukkit.util.BoundingBox;
import org.bukkit.util.Vector;
import org.jetbrains.annotations.Nullable;

/**
 * The implementation behind every entity that is not a player (Paper's {@code CraftEntity}
 * and its many subclasses). Plugins see it through a proxy implementing the entity's API
 * interface ({@code Zombie}, {@code Item}, {@code TNTPrimed}, ...), so {@code instanceof}
 * checks work; methods not defined here throw a named {@code UnsupportedOperationException}.
 *
 * <p>The Rust server owns the entity. Reads fetch its state over the bridge, cached until the
 * next call between the two processes (nothing changes while plugin code runs without
 * calling the server); writes go to the server at once.
 */
public final class HostEntity implements HostProxies.Backed {
    private final CraftServer server;
    private final int id;
    private final UUID uuid;
    private final EntityType type;
    private Map<String, Object> state;
    private long stateEpoch;
    private boolean gone;
    private Object self;
    private @Nullable PermissibleBase perm;

    HostEntity(final CraftServer server, final Map<String, Object> state, final EntityType type) {
        this.server = server;
        this.id = Values.i32(state, "id");
        this.uuid = Values.uuid(state, "uuid");
        this.type = type;
        this.state = state;
        this.stateEpoch = server.epoch();
    }

    @Override
    public void bind(final Object self) {
        this.self = self;
    }

    public Entity self() {
        return (Entity) this.self;
    }

    public int id() {
        return this.id;
    }

    /** The current state, fetched again after anything crossed the bridge. */
    Map<String, Object> state() {
        if (!this.gone && this.stateEpoch != this.server.epoch()) {
            final Object fresh = this.server.call("entity.state", Values.of("id", this.id));
            if (fresh instanceof Map<?, ?>) {
                this.state = Values.map(fresh);
            } else {
                this.gone = true;
                this.state.put("valid", false);
            }
            this.stateEpoch = this.server.epoch();
        }
        return this.state;
    }

    void updateState(final Map<String, Object> state) {
        this.state = state;
        this.stateEpoch = this.server.epoch();
        this.gone = false;
    }

    private void set(final Object... kv) {
        if (this.gone) {
            return;
        }
        final Map<String, Object> args = Values.of(kv);
        args.put("id", this.id);
        final Object fresh = this.server.call("entity.set", args);
        if (fresh instanceof Map<?, ?>) {
            this.updateState(Values.map(fresh));
        }
    }

    private boolean living() {
        return Values.bool(this.state(), "living", false);
    }

    // --- identity -------------------------------------------------------------------------------

    // --- attributes ------------------------------------------------------------------------

    public org.bukkit.attribute.@org.jetbrains.annotations.Nullable AttributeInstance getAttribute(final org.bukkit.attribute.Attribute attribute) {
        return HostAttributeInstance.of(this.server, this.id, attribute, () -> { });
    }

    public void registerAttribute(final org.bukkit.attribute.Attribute attribute) {
        HostAttributeInstance.register(this.server, this.id, attribute);
    }

    public int getEntityId() {
        return this.id;
    }

    public UUID getUniqueId() {
        return this.uuid;
    }

    public EntityType getType() {
        return this.type;
    }

    public Server getServer() {
        return this.server;
    }

    public String getName() {
        final Component custom = this.customName();
        return custom != null ? Components.toLegacy(custom) : Components.toLegacy(Component.translatable(this.type.translationKey()));
    }

    public Component name() {
        final Component custom = this.customName();
        return custom != null ? custom : Component.translatable(this.type.translationKey());
    }

    public Component teamDisplayName() {
        return this.name();
    }

    public String getScoreboardEntryName() {
        return this.uuid.toString();
    }

    public boolean isValid() {
        return !this.gone && Values.bool(this.state(), "valid", false) && !this.isDead();
    }

    public boolean isDead() {
        return this.gone || !Values.bool(this.state(), "valid", false) || Values.bool(this.state(), "dead", false);
    }

    public int getTicksLived() {
        return Values.i32(this.state(), "age", 0);
    }

    public void remove() {
        if (!this.gone) {
            this.server.call("entity.remove", Values.of("id", this.id));
            this.gone = true;
            this.state.put("valid", false);
        }
    }

    public boolean isPersistent() {
        return Values.bool(this.state(), "persistent", false);
    }

    public void setPersistent(final boolean persistent) {
        this.set("persistent", persistent);
    }

    // --- position -------------------------------------------------------------------------------

    public World getWorld() {
        final World world = this.server.getWorld(Values.str(this.state(), "world", "world"));
        return world != null ? world : this.server.getWorlds().getFirst();
    }

    public Location getLocation() {
        final Map<String, Object> s = this.state();
        return new Location(this.getWorld(), Values.f64(s, "x", 0), Values.f64(s, "y", 0), Values.f64(s, "z", 0),
            (float) Values.f64(s, "yaw", 0), (float) Values.f64(s, "pitch", 0));
    }

    public @Nullable Location getLocation(final @Nullable Location loc) {
        if (loc != null) {
            final Location here = this.getLocation();
            loc.setWorld(here.getWorld());
            loc.set(here.getX(), here.getY(), here.getZ());
            loc.setYaw(here.getYaw());
            loc.setPitch(here.getPitch());
        }
        return loc;
    }

    public double getX() {
        return Values.f64(this.state(), "x", 0);
    }

    public double getY() {
        return Values.f64(this.state(), "y", 0);
    }

    public double getZ() {
        return Values.f64(this.state(), "z", 0);
    }

    public float getYaw() {
        return (float) Values.f64(this.state(), "yaw", 0);
    }

    public float getPitch() {
        return (float) Values.f64(this.state(), "pitch", 0);
    }

    public double getWidth() {
        return Values.f64(this.state(), "width", 0.6);
    }

    public double getHeight() {
        return Values.f64(this.state(), "height", 1.8);
    }

    public BoundingBox getBoundingBox() {
        final double x = this.getX(), y = this.getY(), z = this.getZ(), half = this.getWidth() / 2;
        return new BoundingBox(x - half, y, z - half, x + half, y + this.getHeight(), z + half);
    }

    public boolean isOnGround() {
        return Values.bool(this.state(), "on_ground", false);
    }

    public BlockFace getFacing() {
        final int yaw = Math.floorMod((int) Math.floor(this.getYaw() + 45), 360);
        return yaw < 90 ? BlockFace.SOUTH : yaw < 180 ? BlockFace.WEST : yaw < 270 ? BlockFace.NORTH : BlockFace.EAST;
    }

    public Vector getVelocity() {
        final Map<String, Object> s = this.state();
        return new Vector(Values.f64(s, "vx", 0), Values.f64(s, "vy", 0), Values.f64(s, "vz", 0));
    }

    public void setVelocity(final Vector velocity) {
        velocity.checkFinite();
        this.set("vx", velocity.getX(), "vy", velocity.getY(), "vz", velocity.getZ());
    }

    public boolean teleport(final Location location) {
        return this.teleport(location, PlayerTeleportEvent.TeleportCause.PLUGIN);
    }

    public boolean teleport(final Location location, final PlayerTeleportEvent.TeleportCause cause) {
        return this.teleport(location, cause, new TeleportFlag[0]);
    }

    public boolean teleport(final Location location, final TeleportFlag... flags) {
        return this.teleport(location, PlayerTeleportEvent.TeleportCause.PLUGIN, flags);
    }

    public boolean teleport(final Location location, final PlayerTeleportEvent.TeleportCause cause, final TeleportFlag... flags) {
        if (this.gone) {
            return false;
        }
        final Map<String, Object> args = Values.of("id", this.id, "x", location.getX(), "y", location.getY(), "z", location.getZ(),
            "yaw", (double) location.getYaw(), "pitch", (double) location.getPitch());
        if (location.getWorld() != null) {
            args.put("world", location.getWorld().getName());
        }
        try {
            this.server.call("entity.teleport", args);
        } catch (RuntimeException e) {
            this.server.getLogger().warning("Could not teleport " + this.type.getKey() + ": " + e.getMessage());
            return false;
        }
        this.stateEpoch = -1;
        return true;
    }

    public boolean teleport(final Entity destination) {
        return this.teleport(destination.getLocation());
    }

    public boolean teleport(final Entity destination, final PlayerTeleportEvent.TeleportCause cause) {
        return this.teleport(destination.getLocation(), cause);
    }

    public CompletableFuture<Boolean> teleportAsync(final Location location, final PlayerTeleportEvent.TeleportCause cause, final TeleportFlag... flags) {
        final CompletableFuture<Boolean> result = new CompletableFuture<>();
        if (this.server.isPrimaryThread()) {
            result.complete(this.teleport(location, cause, flags));
        } else {
            this.server.runOnMain(() -> result.complete(this.teleport(location, cause, flags)));
        }
        return result;
    }

    public void setRotation(final float yaw, final float pitch) {
        final Location here = this.getLocation();
        here.setYaw(yaw);
        here.setPitch(pitch);
        this.teleport(here);
    }

    public List<Entity> getNearbyEntities(final double x, final double y, final double z) {
        final List<Entity> out = new ArrayList<>(this.getWorld().getNearbyEntities(this.getLocation(), x, y, z));
        out.remove(this.self());
        return out;
    }

    // --- names, fire, gravity -------------------------------------------------------------------

    public @Nullable Component customName() {
        return Components.fromJson(this.state().get("custom_name"));
    }

    public void customName(final @Nullable Component name) {
        if (name == null) {
            this.set("clear_custom_name", true);
        } else {
            this.set("custom_name", Components.toJson(name));
        }
    }

    public @Nullable String getCustomName() {
        final Component name = this.customName();
        return name == null ? null : Components.toLegacy(name);
    }

    public void setCustomName(final @Nullable String name) {
        this.customName(name == null ? null : Components.legacy(name));
    }

    public boolean isCustomNameVisible() {
        return Values.bool(this.state(), "custom_name_visible", false);
    }

    public void setCustomNameVisible(final boolean visible) {
        this.set("custom_name_visible", visible);
    }

    public int getFireTicks() {
        return Values.i32(this.state(), "fire_ticks", 0);
    }

    public void setFireTicks(final int ticks) {
        this.set("fire_ticks", ticks);
    }

    public int getMaxFireTicks() {
        return 1;
    }

    public boolean hasGravity() {
        return !Values.bool(this.state(), "no_gravity", false);
    }

    public void setGravity(final boolean gravity) {
        this.set("gravity", gravity);
    }

    // --- command sender and permissions ---------------------------------------------------------

    public void sendMessage(final String message) {
        // Entities other than players do not receive messages.
    }

    public void sendMessage(final String... messages) {
    }

    public void sendMessage(final @Nullable UUID sender, final String message) {
    }

    public void sendMessage(final @Nullable UUID sender, final String... messages) {
    }

    private PermissibleBase perm() {
        if (this.perm == null) {
            this.perm = new PermissibleBase(null);
        }
        return this.perm;
    }

    public boolean isOp() {
        return false;
    }

    public void setOp(final boolean value) {
        throw new UnsupportedOperationException("Cannot change operator status of an entity");
    }

    public boolean isPermissionSet(final String name) {
        return this.perm().isPermissionSet(name);
    }

    public boolean isPermissionSet(final Permission perm) {
        return this.perm().isPermissionSet(perm);
    }

    public boolean hasPermission(final String name) {
        return this.perm().hasPermission(name);
    }

    public boolean hasPermission(final Permission perm) {
        return this.perm().hasPermission(perm);
    }

    public PermissionAttachment addAttachment(final Plugin plugin, final String name, final boolean value) {
        return this.perm().addAttachment(plugin, name, value);
    }

    public PermissionAttachment addAttachment(final Plugin plugin) {
        return this.perm().addAttachment(plugin);
    }

    public @Nullable PermissionAttachment addAttachment(final Plugin plugin, final String name, final boolean value, final int ticks) {
        return this.perm().addAttachment(plugin, name, value, ticks);
    }

    public @Nullable PermissionAttachment addAttachment(final Plugin plugin, final int ticks) {
        return this.perm().addAttachment(plugin, ticks);
    }

    public void removeAttachment(final PermissionAttachment attachment) {
        this.perm().removeAttachment(attachment);
    }

    public void recalculatePermissions() {
        this.perm().recalculatePermissions();
    }

    public Set<PermissionAttachmentInfo> getEffectivePermissions() {
        return this.perm().getEffectivePermissions();
    }

    // --- metadata and persistent data -----------------------------------------------------------

    public void setMetadata(final String key, final MetadataValue value) {
        this.server.entityMetadata().setMetadata(this.self(), key, value);
    }

    public List<MetadataValue> getMetadata(final String key) {
        return this.server.entityMetadata().getMetadata(this.self(), key);
    }

    public boolean hasMetadata(final String key) {
        return this.server.entityMetadata().hasMetadata(this.self(), key);
    }

    public void removeMetadata(final String key, final Plugin plugin) {
        this.server.entityMetadata().removeMetadata(this.self(), key, plugin);
    }

    /** Kept in memory by the host: entities are not saved with their data yet. */
    public PersistentDataContainer getPersistentDataContainer() {
        return Entities.data(this.server, Values.of("entity", this.id), this.uuid);
    }

    // --- living entities ----------------------------------------------------------------------

    public double getHealth() {
        if (!this.living()) {
            // Item#getHealth (an int) shares the name; item entity health is not modelled.
            throw io.papermc.paperrs.host.Unsupported.method(this.type == EntityType.ITEM ? "Item#getHealth" : "Entity#getHealth");
        }
        return Values.f64(this.state(), "health", 0);
    }

    public void setHealth(final double health) {
        this.set("health", health);
    }

    public double getMaxHealth() {
        return Values.f64(this.state(), "max_health", 20);
    }

    public boolean addPotionEffect(final org.bukkit.potion.PotionEffect effect) {
        return Effects.add(this.server, this.id, effect);
    }

    public boolean addPotionEffect(final org.bukkit.potion.PotionEffect effect, final boolean force) {
        return Effects.add(this.server, this.id, effect);
    }

    public boolean addPotionEffects(final java.util.Collection<org.bukkit.potion.PotionEffect> effects) {
        return Effects.addAll(this.server, this.id, effects);
    }

    public java.util.Collection<org.bukkit.potion.PotionEffect> getActivePotionEffects() {
        return Effects.list(this.server, this.id);
    }

    public boolean hasPotionEffect(final org.bukkit.potion.PotionEffectType type) {
        return Effects.get(this.server, this.id, type) != null;
    }

    public org.bukkit.potion.@Nullable PotionEffect getPotionEffect(final org.bukkit.potion.PotionEffectType type) {
        return Effects.get(this.server, this.id, type);
    }

    public void removePotionEffect(final org.bukkit.potion.PotionEffectType type) {
        Effects.remove(this.server, this.id, type);
    }

    public boolean clearActivePotionEffects() {
        return Effects.clear(this.server, this.id);
    }

    public void setMaxHealth(final double health) {
        this.set("max_health", health);
    }

    public double getAbsorptionAmount() {
        return 0;
    }

    public void damage(final double amount) {
        this.server.call("entity.damage", Values.of("id", this.id, "amount", amount));
        this.stateEpoch = -1;
    }

    public void damage(final double amount, final @Nullable Entity source) {
        this.server.call("entity.damage", Values.of("id", this.id, "amount", amount, "source", Entities.ref(source)));
        this.stateEpoch = -1;
    }

    public double getEyeHeight() {
        return Values.f64(this.state(), "eye_height", this.getHeight() * 0.85);
    }

    public double getEyeHeight(final boolean ignorePose) {
        return this.getEyeHeight();
    }

    public Location getEyeLocation() {
        return this.getLocation().add(0, this.getEyeHeight(), 0);
    }

    public int getRemainingAir() {
        return Values.i32(this.state(), "air", 300);
    }

    public void setRemainingAir(final int ticks) {
        this.set("air", ticks);
    }

    public int getMaximumAir() {
        return 300;
    }

    public int getNoDamageTicks() {
        return Values.i32(this.state(), "no_damage_ticks", 0);
    }

    public void setNoDamageTicks(final int ticks) {
        this.set("no_damage_ticks", ticks);
    }

    public int getMaximumNoDamageTicks() {
        return 20;
    }

    public boolean getRemoveWhenFarAway() {
        return !this.isPersistent();
    }

    public void setRemoveWhenFarAway(final boolean remove) {
        this.setPersistent(!remove);
    }

    public @Nullable EntityEquipment getEquipment() {
        return this.living() ? HostEquipment.of(this) : null;
    }

    /** Equipment of a mob: main hand, off hand, feet, legs, chest, head. */
    ItemStack equipment(final int slot) {
        final List<Object> equipment = Values.list(this.state().get("equipment"));
        return slot < equipment.size() ? HostItemStack.fromValue(equipment.get(slot)) : ItemStack.empty();
    }

    void setEquipment(final int slot, final @Nullable ItemStack item) {
        this.set("equipment", List.of(java.util.Arrays.asList((long) slot, HostItemStack.toValue(item))));
    }

    // ageable

    public boolean isAdult() {
        return !Values.bool(this.state(), "baby", false);
    }

    public void setAdult() {
        this.set("baby", false);
    }

    public void setBaby() {
        this.set("baby", true);
    }

    // mobs

    public @Nullable LivingEntity getTarget() {
        final Object target = this.state().get("target");
        return target instanceof Map<?, ?> t && Entities.of(this.server, Values.map(t)) instanceof LivingEntity living ? living : null;
    }

    public void setTarget(final @Nullable LivingEntity target) {
        if (target == null) {
            this.set("clear_target", true);
        } else {
            this.set("target", Entities.ref(target));
        }
    }

    // --- vehicles and passengers -------------------------------------------------------------

    public List<Entity> getPassengers() {
        final List<Entity> out = new ArrayList<>();
        for (final Object ref : Values.list(this.state().get("passengers"))) {
            final Entity passenger = Entities.of(this.server, Values.map(ref));
            if (passenger != null) {
                out.add(passenger);
            }
        }
        return out;
    }

    @Deprecated
    public @Nullable Entity getPassenger() {
        final List<Entity> passengers = this.getPassengers();
        return passengers.isEmpty() ? null : passengers.get(0);
    }

    @Deprecated
    public boolean setPassenger(final Entity passenger) {
        this.eject();
        return this.addPassenger(passenger);
    }

    public boolean addPassenger(final Entity passenger) {
        if (passenger == null) {
            throw new IllegalArgumentException("Entity cannot be null");
        }
        return this.riding("entity.add_passenger", Values.of("id", this.id, "passenger", Entities.ref(passenger)));
    }

    public boolean removePassenger(final Entity passenger) {
        if (passenger == null) {
            throw new IllegalArgumentException("Entity cannot be null");
        }
        return this.riding("entity.remove_passenger", Values.of("id", this.id, "passenger", Entities.ref(passenger)));
    }

    public boolean isEmpty() {
        return this.getPassengers().isEmpty();
    }

    public boolean eject() {
        return this.riding("entity.eject", Values.of("id", this.id));
    }

    public @Nullable Entity getVehicle() {
        return this.state().get("vehicle") instanceof Map<?, ?> vehicle ? Entities.of(this.server, Values.map(vehicle)) : null;
    }

    public boolean isInsideVehicle() {
        return this.state().get("vehicle") instanceof Map<?, ?>;
    }

    public boolean leaveVehicle() {
        return this.riding("entity.leave_vehicle", Values.of("rider", Values.of("entity", this.id)));
    }

    private boolean riding(final String method, final Map<String, Object> args) {
        if (this.gone) {
            return false;
        }
        final boolean changed = Boolean.TRUE.equals(this.server.call(method, args));
        this.stateEpoch = -1;
        return changed;
    }

    // --- tameable and sittable ------------------------------------------------------------------

    public boolean isTamed() {
        return Values.bool(this.state(), "tamed", false);
    }

    public void setTamed(final boolean tame) {
        if (tame) {
            this.set("tamed", true);
        } else {
            this.set("tamed", false, "owner", "");
        }
    }

    public @Nullable UUID getOwnerUniqueId() {
        return this.state().get("owner") instanceof String uuid && !uuid.isEmpty() ? UUID.fromString(uuid) : null;
    }

    /**
     * {@code Tameable#getOwner} (an {@code AnimalTamer}); {@code Item#getOwner} (the UUID of the
     * only player who may pick the item up) shares the name.
     */
    public @Nullable Object getOwner() {
        if (this.type == EntityType.ITEM) {
            return this.state().get("item_owner") instanceof String uuid && !uuid.isEmpty() ? UUID.fromString(uuid) : null;
        }
        final UUID owner = this.getOwnerUniqueId();
        if (owner == null) {
            return null;
        }
        final org.bukkit.entity.Player online = this.server.getPlayer(owner);
        return online != null ? online : this.server.getOfflinePlayer(owner);
    }

    /** {@code Item#setOwner}: only this player may pick the item up (null: anyone). */
    public void setOwner(final @Nullable UUID owner) {
        this.set("item_owner", owner == null ? "" : owner.toString());
    }

    public void setOwner(final org.bukkit.entity.@Nullable AnimalTamer tamer) {
        if (tamer == null) {
            this.set("tamed", false, "owner", "");
        } else {
            this.set("tamed", true, "owner", tamer.getUniqueId().toString(), "persistent", true);
        }
    }

    public boolean isSitting() {
        return Values.bool(this.state(), "sitting", false);
    }

    public void setSitting(final boolean sitting) {
        this.set("sitting", sitting);
    }

    // --- horses ------------------------------------------------------------------------------------

    public int getDomestication() {
        return Values.i32(this.state(), "temper", 0);
    }

    public void setDomestication(final int value) {
        if (value < 0 || value > this.getMaxDomestication()) {
            throw new IllegalArgumentException("Domestication level (" + value + ") cannot be less than zero or greater than max domestication (" + this.getMaxDomestication() + ")");
        }
        this.set("temper", value);
    }

    public int getMaxDomestication() {
        return 100;
    }

    public double getJumpStrength() {
        return Values.f64(this.state(), "jump_strength", 0.7);
    }

    public void setJumpStrength(final double strength) {
        if (strength < 0) {
            throw new IllegalArgumentException("Jump strength (" + strength + ") cannot be less than zero");
        }
        this.set("jump_strength", strength);
    }

    // --- items ----------------------------------------------------------------------------------

    public ItemStack getItemStack() {
        final HostItemStack stack = HostItemStack.fromValue(this.state().get("item"));
        return stack.mirroring(changed -> this.setItemStack(changed.clone()));
    }

    public void setItemStack(final ItemStack stack) {
        this.set("item", HostItemStack.toValue(stack));
    }

    public int getPickupDelay() {
        return Values.i32(this.state(), "pickup_delay", 0);
    }

    public void setPickupDelay(final int delay) {
        this.set("pickup_delay", Math.min(delay, Short.MAX_VALUE));
    }

    public @Nullable UUID getThrower() {
        final Object thrower = this.state().get("thrower");
        return thrower instanceof String s && !s.isEmpty() ? UUID.fromString(s) : null;
    }

    public void setThrower(final @Nullable UUID thrower) {
        this.set("thrower", thrower == null ? "" : thrower.toString());
    }

    // --- experience orbs ------------------------------------------------------------------------

    public int getExperience() {
        return Values.i32(this.state(), "experience", 0);
    }

    public void setExperience(final int value) {
        this.set("experience", value);
    }

    // --- arrows ---------------------------------------------------------------------------------

    public double getDamage() {
        return Values.f64(this.state(), "damage", 2);
    }

    public void setDamage(final double damage) {
        this.set("damage", damage);
    }

    public boolean isCritical() {
        return Values.bool(this.state(), "critical", false);
    }

    public void setCritical(final boolean critical) {
        this.set("critical", critical);
    }

    public boolean isInBlock() {
        return Values.bool(this.state(), "in_ground", false);
    }

    public AbstractArrow.PickupStatus getPickupStatus() {
        return Values.bool(this.state(), "pickup", false) ? AbstractArrow.PickupStatus.ALLOWED : AbstractArrow.PickupStatus.DISALLOWED;
    }

    public void setPickupStatus(final AbstractArrow.PickupStatus status) {
        this.set("pickup", status == AbstractArrow.PickupStatus.ALLOWED);
    }

    public @Nullable ProjectileSource getShooter() {
        final Object shooter = this.state().get("shooter");
        return shooter instanceof Map<?, ?> s && Entities.of(this.server, Values.map(s)) instanceof ProjectileSource source ? source : null;
    }

    // --- primed TNT -----------------------------------------------------------------------------

    public int getFuseTicks() {
        return Values.i32(this.state(), "fuse", 0);
    }

    public void setFuseTicks(final int ticks) {
        this.set("fuse", ticks);
    }

    // --- falling blocks -------------------------------------------------------------------------

    public Material getMaterial() {
        final String block = Values.str(this.state(), "block", "minecraft:air");
        final int bracket = block.indexOf('[');
        final Material material = Material.matchMaterial(bracket < 0 ? block : block.substring(0, bracket));
        return material == null ? Material.AIR : material;
    }

    // --- merchants: villagers and wandering traders (CraftAbstractVillager, CraftMerchant) ------

    private Map<String, Object> merchantArgs(final Object... kv) {
        final Map<String, Object> args = Values.of(kv);
        args.put("entity", this.id);
        return args;
    }

    private Object villager(final String method, final Object... kv) {
        final Object result = this.server.call(method, this.merchantArgs(kv));
        this.stateEpoch = -1;
        return result;
    }

    public java.util.List<org.bukkit.inventory.MerchantRecipe> getRecipes() {
        return Merchants.recipes(this.server.call("merchant.recipes", this.merchantArgs()));
    }

    public void setRecipes(final java.util.List<org.bukkit.inventory.MerchantRecipe> recipes) {
        this.villager("merchant.set_recipes", "recipes", Merchants.values(recipes));
    }

    public org.bukkit.inventory.MerchantRecipe getRecipe(final int i) {
        return this.getRecipes().get(i);
    }

    public void setRecipe(final int i, final org.bukkit.inventory.MerchantRecipe recipe) {
        java.util.Objects.checkIndex(i, this.getRecipeCount());
        this.villager("merchant.set_recipe", "index", i, "recipe", Merchants.value(recipe));
    }

    public int getRecipeCount() {
        return Values.list(this.server.call("merchant.recipes", this.merchantArgs())).size();
    }

    public boolean isTrading() {
        return this.getTrader() != null;
    }

    public org.bukkit.entity.@Nullable HumanEntity getTrader() {
        final Object trader = this.state().get("trader");
        return trader instanceof String uuid ? this.server.player(UUID.fromString(uuid)) : null;
    }

    /** {@code AbstractVillager#resetOffers}: the trades are worked out again when needed. */
    public void resetOffers() {
        this.villager("villager.reset_offers");
    }

    public org.bukkit.entity.Villager.Profession getProfession() {
        return org.bukkit.Registry.VILLAGER_PROFESSION.getOrThrow(org.bukkit.NamespacedKey.fromString(Values.str(this.state(), "profession", "minecraft:none")));
    }

    public void setProfession(final org.bukkit.entity.Villager.Profession profession) {
        if (profession == null) {
            throw new IllegalArgumentException("Profession cannot be null");
        }
        this.set("profession", profession.getKey().toString());
    }

    public org.bukkit.entity.Villager.Type getVillagerType() {
        return org.bukkit.Registry.VILLAGER_TYPE.getOrThrow(org.bukkit.NamespacedKey.fromString(Values.str(this.state(), "villager_type", "minecraft:plains")));
    }

    public void setVillagerType(final org.bukkit.entity.Villager.Type type) {
        if (type == null) {
            throw new IllegalArgumentException("Type cannot be null");
        }
        this.set("villager_type", type.getKey().toString());
    }

    public int getVillagerLevel() {
        return Values.i32(this.state(), "villager_level", 1);
    }

    public void setVillagerLevel(final int level) {
        if (level < 1 || level > 5) {
            throw new IllegalArgumentException("level (" + level + ") must be between [1, 5]");
        }
        this.set("villager_level", level);
    }

    public int getVillagerExperience() {
        return Values.i32(this.state(), "villager_xp", 0);
    }

    public void setVillagerExperience(final int experience) {
        if (experience < 0) {
            throw new IllegalArgumentException("Experience (" + experience + ") must be positive");
        }
        this.set("villager_xp", experience);
    }

    public boolean increaseLevel(final int amount) {
        if (amount <= 0) {
            throw new IllegalArgumentException("Level earned must be positive");
        }
        return Values.bool(Values.map(this.villager("villager.increase_level", "amount", amount)), "changed", false);
    }

    public boolean addTrades(final int amount) {
        if (amount <= 0) {
            throw new IllegalArgumentException("Number of trades unlocked must be positive");
        }
        return Values.bool(Values.map(this.villager("villager.add_trades", "amount", amount)), "changed", false);
    }

    public int getRestocksToday() {
        return Values.i32(this.state(), "restocks_today", 0);
    }

    public void setRestocksToday(final int restocksToday) {
        this.set("restocks_today", restocksToday);
    }

    public void shakeHead() {
        this.villager("villager.shake_head");
    }

    public void restock() {
        this.villager("villager.restock");
    }

    public void updateDemand() {
        this.villager("villager.update_demand");
    }

    public com.destroystokyo.paper.entity.villager.Reputation getReputation(final UUID uniqueId) {
        if (uniqueId == null) {
            throw new IllegalArgumentException("UUID cannot be null");
        }
        return Merchants.reputation(Values.map(this.server.call("villager.reputations", this.merchantArgs())).get(uniqueId.toString()));
    }

    public Map<UUID, com.destroystokyo.paper.entity.villager.Reputation> getReputations() {
        final Map<UUID, com.destroystokyo.paper.entity.villager.Reputation> out = new java.util.HashMap<>();
        for (final Map.Entry<String, Object> e : Values.map(this.server.call("villager.reputations", this.merchantArgs())).entrySet()) {
            out.put(UUID.fromString(e.getKey()), Merchants.reputation(e.getValue()));
        }
        return out;
    }

    public void setReputation(final UUID uniqueId, final com.destroystokyo.paper.entity.villager.Reputation reputation) {
        if (uniqueId == null || reputation == null) {
            throw new IllegalArgumentException("UUID and reputation cannot be null");
        }
        this.villager("villager.set_reputation", "uuid", uniqueId.toString(), "values", Merchants.reputationValue(reputation));
    }

    public void setReputations(final Map<UUID, com.destroystokyo.paper.entity.villager.Reputation> reputations) {
        reputations.forEach(this::setReputation);
    }

    public void clearReputations() {
        this.villager("villager.clear_reputations");
    }

    public int getDespawnDelay() {
        return Values.i32(this.state(), "despawn_delay", 0);
    }

    public void setDespawnDelay(final int despawnDelay) {
        this.set("despawn_delay", despawnDelay);
    }

    // --- object -------------------------------------------------------------------------------

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostEntity other && other.id == this.id;
    }

    @Override
    public int hashCode() {
        return 31 * 7 + this.id;
    }

    @Override
    public String toString() {
        return "Craft" + this.type.name() + "{id=" + this.id + "}";
    }

    /** {@code Entity#spigot()}: no Spigot extras. */
    public Entity.Spigot spigot() {
        return new Entity.Spigot() {
        };
    }
}
