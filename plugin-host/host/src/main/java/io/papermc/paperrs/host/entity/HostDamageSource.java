package io.papermc.paperrs.host.entity;

import java.util.function.Consumer;
import net.kyori.adventure.pointer.Pointers;
import org.bukkit.Location;
import org.bukkit.damage.DamageSource;
import org.bukkit.damage.DamageType;
import org.bukkit.entity.Entity;
import org.bukkit.event.entity.EntityDamageEvent;
import org.jetbrains.annotations.Nullable;

/** A damage source: the vanilla damage type and who or what caused the damage. */
public record HostDamageSource(DamageType damageType, @Nullable Entity causingEntity, @Nullable Entity directEntity,
                               @Nullable Location damageLocation) implements DamageSource {

    @Override
    public DamageType getDamageType() {
        return this.damageType;
    }

    @Override
    public @Nullable Entity getCausingEntity() {
        return this.causingEntity;
    }

    @Override
    public @Nullable Entity getDirectEntity() {
        return this.directEntity;
    }

    @Override
    public @Nullable Location getDamageLocation() {
        return this.damageLocation;
    }

    @Override
    public @Nullable Location getSourceLocation() {
        if (this.damageLocation != null) {
            return this.damageLocation.clone();
        }
        return this.directEntity != null ? this.directEntity.getLocation() : null;
    }

    @Override
    public boolean isIndirect() {
        return this.causingEntity != null && this.directEntity != null && !this.causingEntity.equals(this.directEntity);
    }

    @Override
    public float getFoodExhaustion() {
        return 0.1f;
    }

    @Override
    public boolean scalesWithDifficulty() {
        return this.causingEntity != null && !(this.causingEntity instanceof org.bukkit.entity.Player);
    }

    @Override
    public Pointers getDamageContext() {
        return Pointers.empty();
    }

    /** {@code DamageSource.builder(type)}. */
    public static final class Builder implements DamageSource.Builder {
        private final DamageType type;
        private @Nullable Entity causing;
        private @Nullable Entity direct;
        private @Nullable Location location;

        public Builder(final DamageType type) {
            this.type = type;
        }

        @Override
        public Builder withCausingEntity(final Entity entity) {
            this.causing = entity;
            return this;
        }

        @Override
        public Builder withDirectEntity(final Entity entity) {
            this.direct = entity;
            return this;
        }

        @Override
        public Builder withDamageLocation(final Location location) {
            this.location = location.clone();
            return this;
        }

        @Override
        public Builder withDamageContext(final Consumer<Pointers.Builder> context) {
            return this;
        }

        @Override
        public DamageSource build() {
            return new HostDamageSource(this.type, this.causing, this.direct, this.location);
        }
    }

    /** The Bukkit damage cause of a vanilla damage type ({@code minecraft:fall}, ...). */
    public static EntityDamageEvent.DamageCause cause(final String type, final boolean byEntity) {
        return switch (type.startsWith("minecraft:") ? type.substring(10) : type) {
            case "player_attack", "mob_attack", "mob_attack_no_aggro", "sting" -> EntityDamageEvent.DamageCause.ENTITY_ATTACK;
            case "player_explosion" -> EntityDamageEvent.DamageCause.ENTITY_EXPLOSION;
            case "explosion", "bad_respawn_point" -> byEntity ? EntityDamageEvent.DamageCause.ENTITY_EXPLOSION : EntityDamageEvent.DamageCause.BLOCK_EXPLOSION;
            case "arrow", "trident", "mob_projectile", "fireball", "unattributed_fireball", "wither_skull", "thrown", "spit", "wind_charge" -> EntityDamageEvent.DamageCause.PROJECTILE;
            case "fall", "ender_pearl" -> EntityDamageEvent.DamageCause.FALL;
            case "out_of_world" -> EntityDamageEvent.DamageCause.VOID;
            case "lava" -> EntityDamageEvent.DamageCause.LAVA;
            case "in_fire", "campfire" -> EntityDamageEvent.DamageCause.FIRE;
            case "on_fire" -> EntityDamageEvent.DamageCause.FIRE_TICK;
            case "drown" -> EntityDamageEvent.DamageCause.DROWNING;
            case "in_wall" -> EntityDamageEvent.DamageCause.SUFFOCATION;
            case "cactus", "sweet_berry_bush", "stalagmite" -> EntityDamageEvent.DamageCause.CONTACT;
            case "hot_floor" -> EntityDamageEvent.DamageCause.HOT_FLOOR;
            case "starve" -> EntityDamageEvent.DamageCause.STARVATION;
            case "magic", "indirect_magic" -> EntityDamageEvent.DamageCause.MAGIC;
            case "wither" -> EntityDamageEvent.DamageCause.WITHER;
            case "lightning_bolt" -> EntityDamageEvent.DamageCause.LIGHTNING;
            case "fly_into_wall" -> EntityDamageEvent.DamageCause.FLY_INTO_WALL;
            case "cramming" -> EntityDamageEvent.DamageCause.CRAMMING;
            case "dragon_breath" -> EntityDamageEvent.DamageCause.DRAGON_BREATH;
            case "freeze" -> EntityDamageEvent.DamageCause.FREEZE;
            case "falling_block", "falling_anvil", "falling_stalactite" -> EntityDamageEvent.DamageCause.FALLING_BLOCK;
            case "thorns" -> EntityDamageEvent.DamageCause.THORNS;
            case "sonic_boom" -> EntityDamageEvent.DamageCause.SONIC_BOOM;
            case "generic_kill" -> EntityDamageEvent.DamageCause.KILL;
            case "outside_border" -> EntityDamageEvent.DamageCause.WORLD_BORDER;
            case "dry_out" -> EntityDamageEvent.DamageCause.DRYOUT;
            default -> EntityDamageEvent.DamageCause.CUSTOM;
        };
    }

    /** Causes Paper reports with {@code EntityDamageByBlockEvent}. */
    public static boolean byBlock(final EntityDamageEvent.DamageCause cause) {
        return switch (cause) {
            case CONTACT, HOT_FLOOR, LAVA, FIRE, BLOCK_EXPLOSION -> true;
            default -> false;
        };
    }
}
