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

    private @Nullable HostEntityScheduler scheduler;

    /** Folia's entity scheduler: tasks on the main thread while the entity is valid. */
    public io.papermc.paper.threadedregions.scheduler.EntityScheduler getScheduler() {
        if (this.scheduler == null) {
            this.scheduler = new HostEntityScheduler(this.self());
        }
        return this.scheduler;
    }

    private org.bukkit.event.entity.@Nullable EntityDamageEvent lastDamageCause;

    public org.bukkit.event.entity.@Nullable EntityDamageEvent getLastDamageCause() {
        return this.lastDamageCause;
    }

    public void setLastDamageCause(final org.bukkit.event.entity.@Nullable EntityDamageEvent event) {
        this.lastDamageCause = event;
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

    /** A chest boat's chest ({@code ChestBoat#getInventory}); other holders' are not supported yet. */
    public org.bukkit.inventory.Inventory getInventory() {
        if (!(this.self instanceof org.bukkit.entity.ChestBoat)) {
            final Class<?> api = this.type.getEntityClass();
            throw io.papermc.paperrs.host.Unsupported.method((api == null ? "Entity" : api.getSimpleName()) + "#getInventory");
        }
        return io.papermc.paperrs.host.inventory.Inventories.entity(this.server, this.getWorld().getName(), this.id);
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
        // A frame or painting faces the way it hangs.
        if (this.state().get("facing") instanceof String facing) {
            return BlockFace.valueOf(facing);
        }
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

    /** Zombies and the like (Zombie#setBaby). */
    public void setBaby(final boolean baby) {
        this.set("baby", baby);
    }

    public boolean isBaby() {
        return !this.isAdult();
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

    /** A tamed animal's owner, a projectile's shooter or a cloud's source (Projectile#getOwnerUniqueId). */
    public @Nullable UUID getOwnerUniqueId() {
        if (this.state().get("owner") instanceof String uuid && !uuid.isEmpty()) {
            return UUID.fromString(uuid);
        }
        if (this.state().containsKey("source")) {
            return this.getSource() instanceof Entity entity ? entity.getUniqueId() : null;
        }
        return this.state().containsKey("shooter") && this.getShooter() instanceof Entity entity ? entity.getUniqueId() : null;
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

    /** Only players and mobs shoot here; other sources (dispensers) clear the shooter. */
    public void setShooter(final @Nullable ProjectileSource shooter) {
        this.set("shooter", shooter instanceof Entity entity ? Entities.ref(entity) : Map.of());
    }

    public boolean hasLeftShooter() {
        return Values.bool(this.state(), "left_shooter", true);
    }

    public void setHasLeftShooter(final boolean leftShooter) {
        this.set("left_shooter", leftShooter);
    }

    public boolean hasBeenShot() {
        return true;
    }

    public boolean doesBounce() {
        return false;
    }

    public void setBounce(final boolean doesBounce) {
        // Deprecated in Bukkit, and without effect there too.
    }

    public <T extends org.bukkit.entity.Projectile> T launchProjectile(final Class<? extends T> projectile, final @Nullable Vector velocity,
                                                                     final java.util.function.@Nullable Consumer<? super T> function) {
        return Entities.launch(this.server, this.self(), projectile, velocity, function);
    }

    // firework rockets (Firework), with CraftFirework's checks

    public org.bukkit.inventory.meta.FireworkMeta getFireworkMeta() {
        return (org.bukkit.inventory.meta.FireworkMeta) this.getItem().getItemMeta();
    }

    /** As CraftFirework: the item takes the meta, and the flight starts again from its power. */
    public void setFireworkMeta(final org.bukkit.inventory.meta.FireworkMeta meta) {
        final ItemStack item = this.getItem();
        item.setItemMeta(meta);
        final java.util.Random random = java.util.concurrent.ThreadLocalRandom.current();
        this.set("item", HostItemStack.toValue(item), "lifetime", 10 * (1 + meta.getPower()) + random.nextInt(6) + random.nextInt(7));
    }

    public boolean isDetonated() {
        return Values.i32(this.state(), "life", 0) > Values.i32(this.state(), "lifetime", 0);
    }

    public void detonate() {
        this.set("detonate", true);
    }

    public boolean setAttachedTo(final @Nullable LivingEntity entity) {
        if (this.isDetonated()) {
            return false;
        }
        this.set("attached", entity == null ? Values.of() : Entities.ref(entity));
        return true;
    }

    public @Nullable LivingEntity getAttachedTo() {
        return this.state().get("attached") instanceof Map<?, ?> ref && Entities.of(this.server, Values.map(ref)) instanceof LivingEntity living ? living : null;
    }

    public boolean setLife(final int ticks) {
        com.google.common.base.Preconditions.checkArgument(ticks >= 0, "ticks must not be negative");
        if (this.isDetonated()) {
            return false;
        }
        this.set("life", ticks);
        return true;
    }

    public boolean setMaxLife(final int ticks) {
        com.google.common.base.Preconditions.checkArgument(ticks > 0, "ticks must be greater than 0");
        if (this.isDetonated()) {
            return false;
        }
        this.set("lifetime", ticks);
        return true;
    }

    public int getTicksFlown() {
        return Values.i32(this.state(), "life", 0);
    }

    public void setTicksFlown(final int ticks) {
        this.set("life", ticks);
    }

    public int getTicksToDetonate() {
        return Values.i32(this.state(), "lifetime", 0);
    }

    public void setTicksToDetonate(final int ticks) {
        this.set("lifetime", ticks);
    }

    public boolean isShotAtAngle() {
        return Values.bool(this.state(), "shot_at_angle", false);
    }

    public void setShotAtAngle(final boolean shotAtAngle) {
        this.set("shot_at_angle", shotAtAngle);
    }

    public java.util.@Nullable UUID getSpawningEntity() {
        return this.state().get("spawning") instanceof String uuid ? java.util.UUID.fromString(uuid) : null;
    }

    // fishing bobbers (FishHook), with CraftFishHook's checks

    private int hookInt(final String key, final int fallback) {
        return Values.i32(this.state(), key, fallback);
    }

    /** {@code FishHook#getState}; other entities' {@code getState} are not this one. */
    public Object getState() {
        if (!(this.self instanceof org.bukkit.entity.FishHook)) {
            throw io.papermc.paperrs.host.Unsupported.method("Entity#getState");
        }
        return org.bukkit.entity.FishHook.HookState.valueOf(Values.str(this.state(), "hook_state", "UNHOOKED"));
    }

    public @Nullable Entity getHookedEntity() {
        return this.state().get("hooked") instanceof Map<?, ?> ref ? Entities.of(this.server, Values.map(ref)) : null;
    }

    public void setHookedEntity(final @Nullable Entity entity) {
        this.set("hooked", entity == null ? Values.of() : Entities.ref(entity));
    }

    public boolean pullHookedEntity() {
        return this.getHookedEntity() != null && Values.bool(Values.map(this.server.call("entity.pull_hooked", Values.of("id", this.id))), "pulled", false);
    }

    public boolean isInOpenWater() {
        return Values.bool(this.state(), "open_water", false);
    }

    public int getMinWaitTime() {
        return this.hookInt("min_wait", 100);
    }

    public void setMinWaitTime(final int minWaitTime) {
        com.google.common.base.Preconditions.checkArgument(minWaitTime >= 0 && minWaitTime <= this.getMaxWaitTime(), "The minimum wait time should be between %s and %s (the maximum wait time)", 0, this.getMaxWaitTime());
        this.set("min_wait", minWaitTime);
    }

    public int getMaxWaitTime() {
        return this.hookInt("max_wait", 600);
    }

    public void setMaxWaitTime(final int maxWaitTime) {
        com.google.common.base.Preconditions.checkArgument(maxWaitTime >= 0 && maxWaitTime >= this.getMinWaitTime(), "The maximum wait time should be between %s and %s (the minimum wait time)", 0, this.getMinWaitTime());
        this.set("max_wait", maxWaitTime);
    }

    public void setWaitTime(final int min, final int max) {
        com.google.common.base.Preconditions.checkArgument(min >= 0 && max >= 0 && min <= max, "The minimum/maximum wait time should be higher than or equal to 0 and the minimum wait time");
        this.set("min_wait", min, "max_wait", max);
    }

    public int getMinLureTime() {
        return this.hookInt("min_lure", 20);
    }

    public void setMinLureTime(final int minLureTime) {
        com.google.common.base.Preconditions.checkArgument(minLureTime >= 0 && minLureTime <= this.getMaxLureTime(), "The minimum lure time (%s) should be between 0 and %s (the maximum wait time)", minLureTime, this.getMaxLureTime());
        this.set("min_lure", minLureTime);
    }

    public int getMaxLureTime() {
        return this.hookInt("max_lure", 80);
    }

    public void setMaxLureTime(final int maxLureTime) {
        com.google.common.base.Preconditions.checkArgument(maxLureTime >= 0 && maxLureTime >= this.getMinLureTime(), "The maximum lure time (%s) should be higher than or equal to 0 and %s (the minimum wait time)", maxLureTime, this.getMinLureTime());
        this.set("max_lure", maxLureTime);
    }

    public void setLureTime(final int min, final int max) {
        com.google.common.base.Preconditions.checkArgument(min >= 0 && max >= 0 && min <= max, "The minimum/maximum lure time should be higher than or equal to 0 and the minimum wait time.");
        this.set("min_lure", min, "max_lure", max);
    }

    public float getMinLureAngle() {
        return (float) Values.f64(this.state(), "min_lure_angle", 0);
    }

    public void setMinLureAngle(final float minLureAngle) {
        com.google.common.base.Preconditions.checkArgument(minLureAngle <= this.getMaxLureAngle(), "The minimum lure angle (%s) should be less than %s (the maximum lure angle)", minLureAngle, this.getMaxLureAngle());
        this.set("min_lure_angle", (double) minLureAngle);
    }

    public float getMaxLureAngle() {
        return (float) Values.f64(this.state(), "max_lure_angle", 360);
    }

    public void setMaxLureAngle(final float maxLureAngle) {
        com.google.common.base.Preconditions.checkArgument(maxLureAngle >= this.getMinLureAngle(), "The minimum lure angle (%s) should be less than %s (the maximum lure angle)", this.getMinLureAngle(), maxLureAngle);
        this.set("max_lure_angle", (double) maxLureAngle);
    }

    public void setLureAngle(final float min, final float max) {
        com.google.common.base.Preconditions.checkArgument(min <= max, "The minimum lure (%s) angle should be less than the maximum lure angle (%s)", min, max);
        this.set("min_lure_angle", (double) min, "max_lure_angle", (double) max);
    }

    public boolean getApplyLure() {
        return Values.bool(this.state(), "apply_lure", true);
    }

    public void setApplyLure(final boolean applyLure) {
        this.set("apply_lure", applyLure);
    }

    public boolean isRainInfluenced() {
        return Values.bool(this.state(), "rain_influenced", true);
    }

    public void setRainInfluenced(final boolean rainInfluenced) {
        this.set("rain_influenced", rainInfluenced);
    }

    public boolean isSkyInfluenced() {
        return Values.bool(this.state(), "sky_influenced", true);
    }

    public void setSkyInfluenced(final boolean skyInfluenced) {
        this.set("sky_influenced", skyInfluenced);
    }

    /** Deprecated in Bukkit and without effect: fish bite after their waits. */
    public double getBiteChance() {
        return -1;
    }

    public void setBiteChance(final double chance) {
        com.google.common.base.Preconditions.checkArgument((chance >= 0 && chance <= 1) || chance == -1, "The bite chance must be between 0 and 1.");
    }

    public int getTimeUntilBite() {
        return this.hookInt("time_until_bite", 0);
    }

    public void setTimeUntilBite(final int ticks) {
        com.google.common.base.Preconditions.checkArgument(ticks > 0, "Time until bite must be positive");
        this.set("time_until_bite", ticks);
    }

    public void resetFishingState() {
        this.set("reset_fishing", true);
    }

    // item frames and paintings (Hanging, ItemFrame, Painting)

    /** The side of the block it hangs on ({@code Attachable}): opposite the way it faces. */
    public BlockFace getAttachedFace() {
        return this.getFacing().getOppositeFace();
    }

    public void setFacingDirection(final BlockFace face) {
        this.setFacingDirection(face, false);
    }

    /**
     * CraftHanging's {@code setFacingDirection}: only where it can then still hang, unless forced;
     * paintings only on walls.
     */
    public boolean setFacingDirection(final BlockFace face, final boolean force) {
        com.google.common.base.Preconditions.checkArgument(face != null && face.isCartesian(), "%s is not a valid facing direction", face);
        if (this.self instanceof org.bukkit.entity.Painting) {
            com.google.common.base.Preconditions.checkArgument(face != BlockFace.UP && face != BlockFace.DOWN, "%s is not a valid facing direction", face);
        }
        this.set("facing", face.name(), "force", force);
        return this.getFacing() == face;
    }

    /** {@code ItemFrame.setItem(item, playSound)}: it holds one of the item. */
    public void setItem(final @Nullable ItemStack item, final boolean playSound) {
        this.set("item", HostItemStack.toValue(item), "sound", playSound);
    }

    public org.bukkit.Rotation getRotation() {
        return org.bukkit.Rotation.values()[Math.floorMod(Values.i32(this.state(), "rotation", 0), 8)];
    }

    public void setRotation(final org.bukkit.Rotation rotation) {
        com.google.common.base.Preconditions.checkArgument(rotation != null, "Rotation cannot be null");
        this.set("rotation", rotation.ordinal());
    }

    public boolean isVisible() {
        return !this.isInvisible();
    }

    public void setVisible(final boolean visible) {
        this.setInvisible(!visible);
    }

    public boolean isFixed() {
        return Values.bool(this.state(), "fixed", false);
    }

    public void setFixed(final boolean fixed) {
        this.set("fixed", fixed);
    }

    public float getItemDropChance() {
        return (float) Values.f64(this.state(), "drop_chance", 1.0);
    }

    public void setItemDropChance(final float chance) {
        com.google.common.base.Preconditions.checkArgument(chance >= 0.0F && chance <= 1.0F, "Chance (%s) outside range [0, 1]", chance);
        this.set("drop_chance", (double) chance);
    }

    public org.bukkit.Art getArt() {
        final org.bukkit.Art art = org.bukkit.Registry.ART.get(org.bukkit.NamespacedKey.fromString(Values.str(this.state(), "art", "minecraft:kebab")));
        return art != null ? art : org.bukkit.Art.KEBAB;
    }

    public boolean setArt(final org.bukkit.Art art) {
        return this.setArt(art, false);
    }

    /** CraftPainting's {@code setArt}: only if it then fits where it hangs, unless forced. */
    public boolean setArt(final org.bukkit.Art art, final boolean force) {
        com.google.common.base.Preconditions.checkArgument(art != null, "Art cannot be null");
        final String key = art.getKey().toString();
        this.set("art", key, "force", force);
        return key.equals(Values.str(this.state(), "art", ""));
    }

    // thrown items (ThrowableProjectile)

    public ItemStack getItem() {
        final ItemStack item = HostItemStack.fromValue(this.state().get("item"));
        // A rocket without an item shows a plain one (`FireworkRocketEntity.getItem`).
        if (item.isEmpty() && this.self instanceof org.bukkit.entity.Firework) {
            return new ItemStack(org.bukkit.Material.FIREWORK_ROCKET);
        }
        return item;
    }

    public void setItem(final @Nullable ItemStack item) {
        this.set("item", HostItemStack.toValue(item));
    }

    // creepers (Creeper)

    public boolean isPowered() {
        return Values.bool(this.state(), "powered", false);
    }

    /** As in Paper, plugins hear of it first ({@code CreeperPowerEvent}, {@code SET_ON} or {@code SET_OFF}). */
    public void setPowered(final boolean powered) {
        if (!(this.self instanceof org.bukkit.entity.Creeper creeper) || powered == this.isPowered()) {
            return;
        }
        final org.bukkit.event.entity.CreeperPowerEvent.PowerCause cause = powered
            ? org.bukkit.event.entity.CreeperPowerEvent.PowerCause.SET_ON : org.bukkit.event.entity.CreeperPowerEvent.PowerCause.SET_OFF;
        if (new org.bukkit.event.entity.CreeperPowerEvent(creeper, cause).callEvent()) {
            this.set("powered", powered);
        }
    }

    // lightning (LightningStrike)

    public boolean isEffect() {
        return Values.bool(this.state(), "effect", false);
    }

    // explosives (Explosive): what primed TNT is

    public float getYield() {
        return 4.0f;
    }

    public boolean isIncendiary() {
        return false;
    }

    // thrown potions (ThrownPotion): what they carry is their item's

    public java.util.Collection<org.bukkit.potion.PotionEffect> getEffects() {
        final List<org.bukkit.potion.PotionEffect> out = new ArrayList<>();
        if (this.getItem().getItemMeta() instanceof org.bukkit.inventory.meta.PotionMeta meta) {
            if (meta.getBasePotionType() != null) {
                out.addAll(meta.getBasePotionType().getPotionEffects());
            }
            out.addAll(meta.getCustomEffects());
        }
        return java.util.Collections.unmodifiableList(out);
    }

    public org.bukkit.inventory.meta.PotionMeta getPotionMeta() {
        return (org.bukkit.inventory.meta.PotionMeta) this.getItem().getItemMeta();
    }

    public void setPotionMeta(final org.bukkit.inventory.meta.PotionMeta meta) {
        final ItemStack item = this.getItem();
        item.setItemMeta(meta);
        this.setItem(item);
    }

    // lingering potions' clouds (AreaEffectCloud)

    public @Nullable ProjectileSource getSource() {
        final Object source = this.state().get("source");
        return source instanceof Map<?, ?> s && Entities.of(this.server, Values.map(s)) instanceof ProjectileSource projectileSource ? projectileSource : null;
    }

    /** Only players and mobs here; other sources clear it. */
    public void setSource(final @Nullable ProjectileSource source) {
        this.set("source", source instanceof Entity entity ? Entities.ref(entity) : Map.of());
    }

    public float getRadius() {
        return (float) Values.f64(this.state(), "radius", 3.0);
    }

    public void setRadius(final float radius) {
        this.set("radius", (double) radius);
    }

    public int getDuration() {
        return Values.i32(this.state(), "duration", 600);
    }

    public void setDuration(final int duration) {
        this.set("duration", duration);
    }

    /** An area effect cloud's wait, or a fishing bobber's wait for a fish ({@code FishHook}). */
    public int getWaitTime() {
        return Values.i32(this.state(), "wait_time", 10);
    }

    public void setWaitTime(final int waitTime) {
        if (this.self instanceof org.bukkit.entity.FishHook) {
            com.google.common.base.Preconditions.checkArgument(waitTime >= 0, "Wait time must be positive");
        }
        this.set("wait_time", waitTime);
    }

    public float getRadiusPerTick() {
        return (float) Values.f64(this.state(), "radius_per_tick", 0);
    }

    public void setRadiusPerTick(final float radiusPerTick) {
        this.set("radius_per_tick", (double) radiusPerTick);
    }

    public float getRadiusOnUse() {
        return (float) Values.f64(this.state(), "radius_on_use", 0);
    }

    public void setRadiusOnUse(final float radiusOnUse) {
        this.set("radius_on_use", (double) radiusOnUse);
    }

    public org.bukkit.potion.@Nullable PotionType getBasePotionType() {
        final org.bukkit.NamespacedKey key = org.bukkit.NamespacedKey.fromString(Values.str(this.state(), "potion", ""));
        for (final org.bukkit.potion.PotionType type : org.bukkit.potion.PotionType.values()) {
            if (type.getKey().equals(key)) {
                return type;
            }
        }
        return null;
    }

    /** Clouds here carry only their potion's effects. */
    public boolean hasCustomEffects() {
        return false;
    }

    public List<org.bukkit.potion.PotionEffect> getCustomEffects() {
        return List.of();
    }

    // fireballs (CraftFireball): they speed up the way they fly, by 0.1 a tick

    public Vector getDirection() {
        return this.getVelocity().normalize();
    }

    public void setDirection(final Vector direction) {
        com.google.common.base.Preconditions.checkArgument(direction != null, "Vector direction cannot be null");
        if (direction.isZero()) {
            this.setVelocity(direction);
            return;
        }
        final double speed = Math.max(this.getVelocity().length(), 0.1);
        this.setVelocity(direction.clone().normalize().multiply(speed));
    }

    public Vector getAcceleration() {
        final Vector velocity = this.getVelocity();
        return velocity.isZero() ? velocity : velocity.normalize().multiply(0.1);
    }

    public void setAcceleration(final Vector acceleration) {
        this.setDirection(acceleration);
    }

    public Vector getPower() {
        return this.getAcceleration();
    }

    public void setPower(final Vector power) {
        this.setDirection(power);
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

    // --- flags any entity has (CraftEntity) ----------------------------------------------------

    public boolean isGlowing() {
        return Values.bool(this.state(), "glowing", false);
    }

    public void setGlowing(final boolean flag) {
        this.set("glowing", flag);
    }

    public boolean isSilent() {
        return Values.bool(this.state(), "silent", false);
    }

    public void setSilent(final boolean flag) {
        this.set("silent", flag);
    }

    public boolean isInvulnerable() {
        return Values.bool(this.state(), "invulnerable", false);
    }

    public boolean isMarkedInvulnerable() {
        return this.isInvulnerable();
    }

    public void setInvulnerable(final boolean flag) {
        this.set("invulnerable", flag);
    }

    public boolean isInvisible() {
        return Values.bool(this.state(), "invisible", false);
    }

    public void setInvisible(final boolean invisible) {
        this.set("invisible", invisible);
    }

    /** Fire shown without burning. ({@code TriState.FALSE}, never showing fire, is taken as not set.) */
    public boolean isVisualFire() {
        return Values.bool(this.state(), "visual_fire", false);
    }

    public net.kyori.adventure.util.TriState getVisualFire() {
        return this.isVisualFire() ? net.kyori.adventure.util.TriState.TRUE : net.kyori.adventure.util.TriState.NOT_SET;
    }

    public void setVisualFire(final boolean fire) {
        this.set("visual_fire", fire);
    }

    public void setVisualFire(final net.kyori.adventure.util.TriState fire) {
        com.google.common.base.Preconditions.checkArgument(fire != null, "TriState cannot be null");
        this.set("visual_fire", fire == net.kyori.adventure.util.TriState.TRUE);
    }

    public int getFreezeTicks() {
        return Values.i32(this.state(), "freeze_ticks", 0);
    }

    public void setFreezeTicks(final int ticks) {
        com.google.common.base.Preconditions.checkArgument(0 <= ticks, "Ticks (%s) cannot be less than 0", ticks);
        this.set("freeze_ticks", ticks);
    }

    public int getMaxFreezeTicks() {
        return 140;
    }

    public boolean isFrozen() {
        return this.getFreezeTicks() >= this.getMaxFreezeTicks();
    }

    public float getFallDistance() {
        return (float) Values.f64(this.state(), "fall_distance", 0);
    }

    public void setFallDistance(final float distance) {
        this.set("fall_distance", (double) distance);
    }

    public void setTicksLived(final int value) {
        com.google.common.base.Preconditions.checkArgument(value > 0, "Age value (%s) must be greater than 0", value);
        this.set("ticks_lived", value);
    }

    public int getPortalCooldown() {
        return Values.i32(this.state(), "portal_cooldown", 0);
    }

    public void setPortalCooldown(final int cooldown) {
        com.google.common.base.Preconditions.checkArgument(cooldown >= 0, "Portal cooldown (%s) must be beq 0", cooldown);
        this.set("portal_cooldown", cooldown);
    }

    private List<String> tags() {
        final List<String> tags = new ArrayList<>();
        for (final Object tag : Values.list(this.state().get("tags"))) {
            tags.add(String.valueOf(tag));
        }
        return tags;
    }

    /** The entity's own tags, as CraftEntity hands out: adding to or removing from the set changes them. */
    public Set<String> getScoreboardTags() {
        return new java.util.AbstractSet<>() {
            @Override
            public java.util.Iterator<String> iterator() {
                final java.util.Iterator<String> tags = HostEntity.this.tags().iterator();
                return new java.util.Iterator<>() {
                    private @Nullable String last;

                    @Override
                    public boolean hasNext() {
                        return tags.hasNext();
                    }

                    @Override
                    public String next() {
                        return this.last = tags.next();
                    }

                    @Override
                    public void remove() {
                        if (this.last == null) {
                            throw new IllegalStateException();
                        }
                        HostEntity.this.removeScoreboardTag(this.last);
                        this.last = null;
                    }
                };
            }

            @Override
            public int size() {
                return HostEntity.this.tags().size();
            }

            @Override
            public boolean contains(final Object o) {
                return HostEntity.this.tags().contains(o);
            }

            @Override
            public boolean add(final String tag) {
                return HostEntity.this.addScoreboardTag(tag);
            }

            @Override
            public boolean remove(final Object o) {
                return o instanceof String tag && HostEntity.this.removeScoreboardTag(tag);
            }
        };
    }

    public boolean addScoreboardTag(final String tag) {
        final List<String> tags = this.tags();
        if (tags.size() >= 1024 || tags.contains(tag)) {
            return false;
        }
        this.set("add_tag", tag);
        return true;
    }

    public boolean removeScoreboardTag(final String tag) {
        if (!this.tags().contains(tag)) {
            return false;
        }
        this.set("remove_tag", tag);
        return true;
    }

    public org.bukkit.entity.Pose getPose() {
        return this.isDead() ? org.bukkit.entity.Pose.DYING : org.bukkit.entity.Pose.STANDING;
    }

    public boolean isSneaking() {
        return false;
    }

    public org.bukkit.entity.SpawnCategory getSpawnCategory() {
        return switch (Values.str(this.state(), "category", "MISC")) {
            case "MONSTER" -> org.bukkit.entity.SpawnCategory.MONSTER;
            case "CREATURE" -> org.bukkit.entity.SpawnCategory.ANIMAL;
            case "AMBIENT" -> org.bukkit.entity.SpawnCategory.AMBIENT;
            case "AXOLOTLS" -> org.bukkit.entity.SpawnCategory.AXOLOTL;
            case "WATER_CREATURE" -> org.bukkit.entity.SpawnCategory.WATER_ANIMAL;
            case "WATER_AMBIENT" -> org.bukkit.entity.SpawnCategory.WATER_AMBIENT;
            case "UNDERGROUND_WATER_CREATURE" -> org.bukkit.entity.SpawnCategory.WATER_UNDERGROUND_CREATURE;
            default -> org.bukkit.entity.SpawnCategory.MISC;
        };
    }

    public org.bukkit.SoundCategory getSoundCategory() {
        return "MONSTER".equals(Values.str(this.state(), "category", "MISC")) ? org.bukkit.SoundCategory.HOSTILE : org.bukkit.SoundCategory.NEUTRAL;
    }

    public org.bukkit.block.PistonMoveReaction getPistonMoveReaction() {
        return org.bukkit.block.PistonMoveReaction.MOVE;
    }

    // where it is

    public boolean isInWater() {
        return Values.bool(this.state(), "in_water", false);
    }

    public boolean isUnderWater() {
        return Values.bool(this.state(), "under_water", false);
    }

    public boolean isInLava() {
        return Values.bool(this.state(), "in_lava", false);
    }

    public boolean isInRain() {
        return Values.bool(this.state(), "in_rain", false);
    }

    public boolean isInBubbleColumn() {
        return Values.bool(this.state(), "in_bubble_column", false);
    }

    public boolean isInPowderedSnow() {
        return Values.bool(this.state(), "in_powder_snow", false);
    }

    public boolean isInWorld() {
        return !this.gone && Values.bool(this.state(), "valid", false);
    }

    /** Every entity in a loaded chunk ticks. */
    public boolean isTicking() {
        return this.isInWorld();
    }

    public Set<org.bukkit.entity.Player> getTrackedBy() {
        final Set<org.bukkit.entity.Player> players = new java.util.HashSet<>();
        for (final Object uuid : Values.list(this.state().get("tracked_by"))) {
            final org.bukkit.entity.Player player = this.server.getPlayer(UUID.fromString(String.valueOf(uuid)));
            if (player != null) {
                players.add(player);
            }
        }
        return players;
    }

    public Set<org.bukkit.entity.Player> getTrackedPlayers() {
        return this.getTrackedBy();
    }

    public boolean isTrackedBy(final org.bukkit.entity.Player player) {
        return this.getTrackedBy().contains(player);
    }

    // how it came to be

    public org.bukkit.event.entity.CreatureSpawnEvent.SpawnReason getEntitySpawnReason() {
        final String reason = Values.str(this.state(), "spawn_reason", null);
        if (reason != null) {
            try {
                return org.bukkit.event.entity.CreatureSpawnEvent.SpawnReason.valueOf(reason);
            } catch (IllegalArgumentException ignored) {
                // a reason of a newer server
            }
        }
        return org.bukkit.event.entity.CreatureSpawnEvent.SpawnReason.DEFAULT;
    }

    public boolean fromMobSpawner() {
        return "SPAWNER".equals(Values.str(this.state(), "spawn_reason", null));
    }

    /** Where it was first spawned is not recorded. */
    public @Nullable Location getOrigin() {
        return null;
    }

    public void playEffect(final org.bukkit.EntityEffect type) {
        com.google.common.base.Preconditions.checkArgument(type != null, "Type cannot be null");
        if (!this.gone && type.isApplicableTo(this.self())) {
            this.server.call("entity.event", Values.of("id", this.id, "status", (int) type.getData()));
        }
    }

    // --- living entities and mobs (CraftLivingEntity, CraftMob) ---------------------------------

    public boolean hasAI() {
        return Values.bool(this.state(), "ai", false);
    }

    public void setAI(final boolean ai) {
        this.set("ai", ai);
    }

    public boolean isAware() {
        return Values.bool(this.state(), "aware", true);
    }

    public void setAware(final boolean aware) {
        this.set("aware", aware);
    }

    public boolean isLeftHanded() {
        return Values.bool(this.state(), "left_handed", false);
    }

    public void setLeftHanded(final boolean leftHanded) {
        this.set("left_handed", leftHanded);
    }

    public boolean getCanPickupItems() {
        return Values.bool(this.state(), "can_pick_up_loot", false);
    }

    public void setCanPickupItems(final boolean pickup) {
        this.set("can_pick_up_loot", pickup);
    }

    public boolean isAggressive() {
        return this.state().get("target") != null;
    }

    public int getPossibleExperienceReward() {
        return Values.i32(this.state(), "experience_reward", 0);
    }

    public int getHeadRotationSpeed() {
        return 10;
    }

    public int getMaxHeadPitch() {
        return 40;
    }

    public double getLastDamage() {
        return Values.f64(this.state(), "last_damage", 0);
    }

    public void setLastDamage(final double damage) {
        this.set("last_damage", damage);
    }

    public int getNoActionTicks() {
        return Values.i32(this.state(), "no_action_ticks", 0);
    }

    public void setNoActionTicks(final int ticks) {
        this.set("no_action_ticks", ticks);
    }

    /** The player who hurt it in the last five seconds. */
    public org.bukkit.entity.@Nullable Player getKiller() {
        final String killer = Values.str(this.state(), "killer", null);
        return killer == null ? null : this.server.getPlayer(UUID.fromString(killer));
    }

    /** Leads are not supported yet: nothing is leashed. */
    public boolean isLeashed() {
        return false;
    }

    public Entity getLeashHolder() {
        throw new IllegalStateException("Entity not leashed");
    }

    public boolean isGliding() {
        return false;
    }

    public boolean isSwimming() {
        return false;
    }

    public boolean isRiptiding() {
        return false;
    }

    public boolean isSleeping() {
        return false;
    }

    public boolean isClimbing() {
        return false;
    }

    public void swingMainHand() {
        this.server.call("entity.swing", Values.of("id", this.id, "hand", 0));
    }

    public void swingOffHand() {
        this.server.call("entity.swing", Values.of("id", this.id, "hand", 1));
    }

    public float getBodyYaw() {
        return this.getYaw();
    }

    public void setBodyYaw(final float bodyYaw) {
        this.setRotation(bodyYaw, this.getPitch());
    }

    public void knockback(final double strength, final double directionX, final double directionZ) {
        com.google.common.base.Preconditions.checkArgument(strength > 0, "Knockback strength must be > 0");
        // LivingEntity.knockback: half the current motion, pushed away from the direction.
        final Vector now = this.getVelocity();
        final Vector push = new Vector(directionX, 0, directionZ).normalize().multiply(strength);
        this.setVelocity(new Vector(now.getX() / 2 - push.getX(), this.isOnGround() ? Math.min(0.4, now.getY() / 2 + strength) : now.getY(), now.getZ() / 2 - push.getZ()));
    }

    /** Turns it to look at a point at once (Paper's Mob#lookAt turns its head over the next ticks). */
    public void lookAt(final double x, final double y, final double z) {
        final Location eye = this.getEyeLocation();
        final double dx = x - eye.getX(), dy = y - eye.getY(), dz = z - eye.getZ();
        final float yaw = (float) (Math.toDegrees(Math.atan2(dz, dx)) - 90);
        final float pitch = (float) -Math.toDegrees(Math.atan2(dy, Math.sqrt(dx * dx + dz * dz)));
        this.setRotation(yaw, pitch);
    }

    public void lookAt(final double x, final double y, final double z, final float headRotationSpeed, final float maxHeadPitch) {
        this.lookAt(x, y, z);
    }

    public void lookAt(final Location location) {
        this.lookAt(location.getX(), location.getY(), location.getZ());
    }

    public void lookAt(final Location location, final float headRotationSpeed, final float maxHeadPitch) {
        this.lookAt(location);
    }

    public void lookAt(final Entity entity) {
        this.lookAt(entity instanceof LivingEntity living ? living.getEyeLocation() : entity.getLocation());
    }

    public void lookAt(final Entity entity, final float headRotationSpeed, final float maxHeadPitch) {
        this.lookAt(entity);
    }

    private static org.bukkit.@Nullable Sound sound(final @Nullable String key) {
        final org.bukkit.NamespacedKey name = key == null ? null : org.bukkit.NamespacedKey.fromString(key);
        return name == null ? null : org.bukkit.Registry.SOUND_EVENT.get(name);
    }

    public org.bukkit.@Nullable Sound getHurtSound() {
        return sound(Values.str(this.state(), "hurt_sound", null));
    }

    public org.bukkit.@Nullable Sound getDeathSound() {
        return sound(Values.str(this.state(), "death_sound", null));
    }

    public org.bukkit.@Nullable Sound getAmbientSound() {
        return sound(Values.str(this.state(), "ambient_sound", null));
    }

    public float getSoundVolume() {
        return 1.0f;
    }

    public float getSoundPitch() {
        return this.isAdult() ? 1.0f : 1.5f;
    }

    // --- ageable and animals (CraftAgeable, CraftAnimals) ---------------------------------------

    /** Animals and villagers have an age; zombies, piglins and the like are just babies or not. */
    private boolean ages() {
        return Values.bool(this.state(), "ages", false);
    }

    public int getAge() {
        if (!this.ages()) {
            return this.isAdult() ? 0 : -1;
        }
        return Values.i32(this.state(), "mob_age", 0);
    }

    public void setAge(final int age) {
        this.set("mob_age", age);
    }

    public boolean getAgeLock() {
        return Values.bool(this.state(), "age_locked", false);
    }

    public void setAgeLock(final boolean lock) {
        if (this.ages()) {
            this.set("age_locked", lock);
        }
    }

    public boolean canBreed() {
        return this.ages() && this.getAge() == 0;
    }

    public void setBreed(final boolean breed) {
        if (!this.ages()) {
            return;
        }
        if (breed) {
            this.setAge(0);
        } else if (this.getAge() == 0) {
            this.setAge(6000);
        }
    }

    public @Nullable UUID getBreedCause() {
        final String cause = Values.str(this.state(), "love_cause", null);
        return cause == null ? null : UUID.fromString(cause);
    }

    public void setBreedCause(final @Nullable UUID uuid) {
        this.set("love_cause", uuid == null ? "" : uuid.toString());
    }

    public boolean isLoveMode() {
        return this.getLoveModeTicks() > 0;
    }

    public int getLoveModeTicks() {
        return Values.i32(this.state(), "love_ticks", 0);
    }

    public void setLoveModeTicks(final int ticks) {
        com.google.common.base.Preconditions.checkArgument(ticks >= 0, "Love mode ticks must be positive or 0");
        this.set("love_ticks", ticks);
    }

    public boolean isBreedItem(final @Nullable ItemStack stack) {
        return stack != null && !stack.isEmpty() && this.isBreedItem(stack.getType());
    }

    public boolean isBreedItem(final @Nullable Material material) {
        if (material == null || !material.isItem() || material.isAir()) {
            return false;
        }
        return Boolean.TRUE.equals(this.server.call("entity.breed_item", Values.of("id", this.id, "item", material.getKey().toString())));
    }

    // --- sheep (CraftSheep) ----------------------------------------------------------------------

    private void sheepOnly(final String method) {
        if (this.type != EntityType.SHEEP) {
            throw io.papermc.paperrs.host.Unsupported.method(this.type.getEntityClass().getSimpleName() + "#" + method);
        }
    }

    public boolean isSheared() {
        return Values.bool(this.state(), "sheared", false);
    }

    public void setSheared(final boolean flag) {
        this.sheepOnly("setSheared");
        this.set("sheared", flag);
    }

    /** A sheep's wool (a DyeColor), a llama's coat (Llama.Color) or a horse's (Horse.Color). */
    public @Nullable Object getColor() {
        final int variant = this.variantNumber();
        return switch (this.type) {
            case SHEEP -> org.bukkit.DyeColor.getByWoolData((byte) variant);
            case LLAMA, TRADER_LLAMA -> byOrdinal(org.bukkit.entity.Llama.Color.values(), variant);
            case HORSE -> byOrdinal(org.bukkit.entity.Horse.Color.values(), variant & 0xFF);
            default -> throw io.papermc.paperrs.host.Unsupported.method(this.type.getEntityClass().getSimpleName() + "#getColor");
        };
    }

    public void setColor(final org.bukkit.@Nullable DyeColor color) {
        this.sheepOnly("setColor");
        com.google.common.base.Preconditions.checkArgument(color != null, "color cannot be null");
        this.set("variant", (int) color.getWoolData());
    }

    // --- variants (CraftCat, CraftWolf, CraftCow, CraftRabbit, CraftHorse, ...) ------------------

    private int variantNumber() {
        return Values.i32(this.state(), "variant", 0);
    }

    private static <E> E byOrdinal(final E[] values, final int i) {
        return values[i >= 0 && i < values.length ? i : 0];
    }

    /** An entry of the mob's variant registry (cats, chickens, cows, frogs, pigs, wolves). */
    private <T extends org.bukkit.Keyed> T registryVariant(final io.papermc.paper.registry.RegistryKey<T> key) {
        final String name = Values.str(this.state(), "variant_key", null);
        if (name == null) {
            throw new IllegalStateException(this.type.getKey() + " has no variant");
        }
        return io.papermc.paper.registry.RegistryAccess.registryAccess().getRegistry(key).getOrThrow(org.bukkit.NamespacedKey.fromString(name));
    }

    private void setRegistryVariant(final org.bukkit.Keyed variant) {
        com.google.common.base.Preconditions.checkArgument(variant != null, "variant cannot be null");
        this.set("variant_key", variant.getKey().toString());
    }

    private void setNumberedVariant(final Enum<?> variant) {
        com.google.common.base.Preconditions.checkArgument(variant != null, "variant cannot be null");
        this.set("variant", variant.ordinal());
    }

    public Object getVariant() {
        return switch (this.type) {
            case WOLF -> this.registryVariant(io.papermc.paper.registry.RegistryKey.WOLF_VARIANT);
            case COW -> this.registryVariant(io.papermc.paper.registry.RegistryKey.COW_VARIANT);
            case PIG -> this.registryVariant(io.papermc.paper.registry.RegistryKey.PIG_VARIANT);
            case CHICKEN -> this.registryVariant(io.papermc.paper.registry.RegistryKey.CHICKEN_VARIANT);
            case FROG -> this.registryVariant(io.papermc.paper.registry.RegistryKey.FROG_VARIANT);
            case ZOMBIE_NAUTILUS -> this.registryVariant(io.papermc.paper.registry.RegistryKey.ZOMBIE_NAUTILUS_VARIANT);
            case PARROT -> byOrdinal(org.bukkit.entity.Parrot.Variant.values(), this.variantNumber());
            case AXOLOTL -> byOrdinal(org.bukkit.entity.Axolotl.Variant.values(), this.variantNumber());
            case MOOSHROOM -> byOrdinal(org.bukkit.entity.MushroomCow.Variant.values(), this.variantNumber());
            default -> throw io.papermc.paperrs.host.Unsupported.method(this.type.getEntityClass().getSimpleName() + "#getVariant");
        };
    }

    public void setVariant(final org.bukkit.entity.Wolf.Variant variant) {
        this.setRegistryVariant(variant);
    }

    public void setVariant(final org.bukkit.entity.Cow.Variant variant) {
        this.setRegistryVariant(variant);
    }

    public void setVariant(final org.bukkit.entity.Pig.Variant variant) {
        this.setRegistryVariant(variant);
    }

    public void setVariant(final org.bukkit.entity.Chicken.Variant variant) {
        this.setRegistryVariant(variant);
    }

    public void setVariant(final org.bukkit.entity.Frog.Variant variant) {
        this.setRegistryVariant(variant);
    }

    public void setVariant(final org.bukkit.entity.ZombieNautilus.Variant variant) {
        this.setRegistryVariant(variant);
    }

    public void setVariant(final org.bukkit.entity.Parrot.Variant variant) {
        this.setNumberedVariant(variant);
    }

    public void setVariant(final org.bukkit.entity.Axolotl.Variant variant) {
        this.setNumberedVariant(variant);
    }

    public void setVariant(final org.bukkit.entity.MushroomCow.Variant variant) {
        this.setNumberedVariant(variant);
    }

    public org.bukkit.entity.Cat.Type getCatType() {
        return this.registryVariant(io.papermc.paper.registry.RegistryKey.CAT_VARIANT);
    }

    public void setCatType(final org.bukkit.entity.Cat.Type type) {
        this.setRegistryVariant(type);
    }

    /** Vanilla's rabbit types are numbered as Bukkit's, but for the killer bunny (99). */
    public org.bukkit.entity.Rabbit.Type getRabbitType() {
        final int type = this.variantNumber();
        return type == 99 ? org.bukkit.entity.Rabbit.Type.THE_KILLER_BUNNY : byOrdinal(org.bukkit.entity.Rabbit.Type.values(), type);
    }

    public void setRabbitType(final org.bukkit.entity.Rabbit.Type type) {
        com.google.common.base.Preconditions.checkArgument(type != null, "type cannot be null");
        this.set("variant", type == org.bukkit.entity.Rabbit.Type.THE_KILLER_BUNNY ? 99 : type.ordinal());
    }

    public org.bukkit.entity.Fox.Type getFoxType() {
        return byOrdinal(org.bukkit.entity.Fox.Type.values(), this.variantNumber());
    }

    public void setFoxType(final org.bukkit.entity.Fox.Type type) {
        this.setNumberedVariant(type);
    }

    public void setColor(final org.bukkit.entity.Llama.Color color) {
        this.setNumberedVariant(color);
    }

    /** A horse's coat (the low byte of its variant) and markings (the next). */
    public void setColor(final org.bukkit.entity.Horse.Color color) {
        com.google.common.base.Preconditions.checkArgument(color != null, "color cannot be null");
        this.set("variant", color.ordinal() | this.variantNumber() & 0xFF00);
    }

    public org.bukkit.entity.Horse.Style getStyle() {
        return byOrdinal(org.bukkit.entity.Horse.Style.values(), this.variantNumber() >> 8 & 0xFF);
    }

    public void setStyle(final org.bukkit.entity.Horse.Style style) {
        com.google.common.base.Preconditions.checkArgument(style != null, "style cannot be null");
        this.set("variant", this.variantNumber() & 0xFF | style.ordinal() << 8);
    }

    /** Wolves' and cats' collars. */
    public org.bukkit.DyeColor getCollarColor() {
        return org.bukkit.DyeColor.getByWoolData((byte) Values.i32(this.state(), "collar", 14));
    }

    public void setCollarColor(final org.bukkit.DyeColor color) {
        com.google.common.base.Preconditions.checkArgument(color != null, "color cannot be null");
        this.set("collar", (int) color.getWoolData());
    }

    public boolean readyToBeSheared() {
        return !this.isDead() && !this.isSheared() && this.isAdult();
    }

    public void shear() {
        this.shear(net.kyori.adventure.sound.Sound.Source.PLAYER);
    }

    public void shear(final net.kyori.adventure.sound.Sound.Source source) {
        this.sheepOnly("shear");
        final Object fresh = this.server.call("entity.shear", Values.of("id", this.id));
        if (fresh instanceof Map<?, ?>) {
            this.updateState(Values.map(fresh));
        }
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
