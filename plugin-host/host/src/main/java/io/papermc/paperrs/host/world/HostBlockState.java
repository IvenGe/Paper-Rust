package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.Components;

import io.papermc.paperrs.host.HostProxies;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.inventory.Inventories;
import java.util.List;
import java.util.Map;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.World;
import org.bukkit.block.Barrel;
import org.bukkit.block.BlastFurnace;
import org.bukkit.block.Block;
import org.bukkit.block.BlockState;
import org.bukkit.block.Chest;
import org.bukkit.block.Dispenser;
import org.bukkit.block.Dropper;
import org.bukkit.block.EnderChest;
import org.bukkit.block.Furnace;
import org.bukkit.block.Hopper;
import org.bukkit.block.Smoker;
import org.bukkit.block.data.BlockData;
import org.bukkit.event.inventory.InventoryType;
import org.bukkit.inventory.Inventory;
import org.jetbrains.annotations.Nullable;

/**
 * {@code Block#getState()}: a snapshot of the block at a position, implementing the block's
 * state interface for the containers Paper-RS supports ({@code Chest}, {@code Barrel},
 * {@code Furnace}, {@code Smoker}, {@code BlastFurnace}, {@code EnderChest}, {@code Hopper},
 * {@code Dispenser}, {@code Dropper}); other blocks are plain {@code BlockState}s. The block
 * data is a snapshot that {@code update()} writes back, as in {@code CraftBlockState}; container
 * inventories are live (the server keeps the items in the block entity).
 */
public final class HostBlockState implements HostProxies.Backed {
    private final HostBlock block;
    private final Class<? extends BlockState> iface;
    /** The snapshot, as {@code minecraft:name[property=value,...]}. */
    private String data;
    private Material type;
    private Object self;

    private HostBlockState(final HostBlock block, final Class<? extends BlockState> iface, final String data) {
        this.block = block;
        this.iface = iface;
        this.setData(data);
    }

    static BlockState of(final HostBlock block) {
        return of(block, block.stateString());
    }

    /** A snapshot of {@code block} holding {@code state} (for events about a state the world no longer has). */
    public static BlockState of(final HostBlock block, final String state) {
        final String name = state.indexOf('[') < 0 ? state : state.substring(0, state.indexOf('['));
        final String path = name.substring(name.indexOf(':') + 1);
        final Class<? extends BlockState> iface = path.endsWith("_hanging_sign") ? org.bukkit.block.HangingSign.class
            : path.endsWith("_sign") ? org.bukkit.block.Sign.class
            : switch (path) {
            case "chest", "trapped_chest" -> Chest.class;
            case "barrel" -> Barrel.class;
            case "furnace" -> Furnace.class;
            case "smoker" -> Smoker.class;
            case "blast_furnace" -> BlastFurnace.class;
            case "ender_chest" -> EnderChest.class;
            case "hopper" -> Hopper.class;
            case "dispenser" -> Dispenser.class;
            case "dropper" -> Dropper.class;
            default -> BlockState.class;
        };
        return HostProxies.create(new HostBlockState(block, iface, state), iface);
    }

    private void setData(final String state) {
        this.data = state;
        final int bracket = state.indexOf('[');
        final Material material = Material.matchMaterial(bracket < 0 ? state : state.substring(0, bracket));
        this.type = material == null ? Material.AIR : material;
    }

    @Override
    public void bind(final Object self) {
        this.self = self;
    }

    public Block getBlock() {
        return this.block;
    }

    public Material getType() {
        return this.type;
    }

    public void setType(final Material type) {
        if (type == null || !type.isBlock()) {
            throw new IllegalArgumentException(type + " is not a block");
        }
        if (type != this.type) {
            this.setData(HostBlockData.defaults(type).getAsString());
        }
    }

    public BlockData getBlockData() {
        return HostBlockData.ofState(this.data);
    }

    public void setBlockData(final BlockData data) {
        if (data == null) {
            throw new IllegalArgumentException("BlockData cannot be null");
        }
        this.setData(HostBlockData.impl(data).getAsString(false));
    }

    public World getWorld() {
        return this.block.getWorld();
    }

    public int getX() {
        return this.block.getX();
    }

    public int getY() {
        return this.block.getY();
    }

    public int getZ() {
        return this.block.getZ();
    }

    public Location getLocation() {
        return this.block.getLocation();
    }

    public @Nullable Location getLocation(final @Nullable Location loc) {
        return this.block.getLocation(loc);
    }

    public byte getLightLevel() {
        return this.block.getLightLevel();
    }

    public boolean isPlaced() {
        return true;
    }

    public boolean update() {
        return this.update(false, true);
    }

    public boolean update(final boolean force) {
        return this.update(force, true);
    }

    /**
     * Writes the snapshot's block data to the world, and a sign's text if it was changed.
     * Unless forced, only while the world still has the same type of block there.
     */
    public boolean update(final boolean force, final boolean applyPhysics) {
        if (!force && this.block.getType() != this.type) {
            return false;
        }
        this.block.setState(this.data, applyPhysics);
        if (this.signChanged && this.sign != null) {
            final HostWorld world = (HostWorld) this.block.getWorld();
            final Map<String, Object> args = new java.util.HashMap<>(this.sign);
            args.putAll(Values.of("world", world.getName(), "x", this.getX(), "y", this.getY(), "z", this.getZ()));
            world.server().call("world.set_sign", args);
            this.signChanged = false;
        }
        return true;
    }

    // --- signs (CraftSign: a snapshot of both sides, written back by update) ------------------

    private @Nullable Map<String, Object> sign;
    private boolean signChanged;
    private java.util.@Nullable UUID allowedEditor;

    /** The sign's text as read when first asked: {front, back: {lines, color, glowing}, waxed}. */
    Map<String, Object> sign() {
        if (this.sign == null) {
            final HostWorld world = (HostWorld) this.block.getWorld();
            final Object value = world.server().call("world.sign", Values.of("world", world.getName(), "x", this.getX(), "y", this.getY(), "z", this.getZ()));
            final Map<String, Object> sign = new java.util.HashMap<>();
            final Map<String, Object> read = value instanceof Map<?, ?> m ? Values.map(m) : Map.of();
            for (final String side : new String[] {"front", "back"}) {
                final Map<String, Object> text = new java.util.HashMap<>(Values.map(read.getOrDefault(side, Map.of())));
                final List<Object> lines = new java.util.ArrayList<>(Values.list(text.get("lines")));
                while (lines.size() < 4) {
                    lines.add(Components.toJson(net.kyori.adventure.text.Component.empty()));
                }
                text.put("lines", lines);
                text.putIfAbsent("color", "black");
                text.putIfAbsent("glowing", false);
                sign.put(side, text);
            }
            sign.put("waxed", Values.bool(read, "waxed", false));
            this.sign = sign;
        }
        return this.sign;
    }

    void signChanged() {
        this.signChanged = true;
    }

    public org.bukkit.block.sign.SignSide getSide(final org.bukkit.block.sign.Side side) {
        com.google.common.base.Preconditions.checkArgument(side != null, "side == null");
        return new HostSignSide(this, side == org.bukkit.block.sign.Side.FRONT ? "front" : "back");
    }

    public org.bukkit.block.sign.SignSide getTargetSide(final org.bukkit.entity.Player player) {
        return this.getSide(this.getInteractableSideFor(player.getX(), player.getZ()));
    }

    /** {@code SignBlockEntity.isFacingFrontText}: which side faces a point. */
    public org.bukkit.block.sign.Side getInteractableSideFor(final double x, final double z) {
        final Map<String, String> props = io.papermc.paperrs.host.data.HostData.get().properties(io.papermc.paperrs.host.data.HostData.get().parseState(this.data));
        final String name = this.type.getKey().getKey();
        double cx = 0.5;
        double cz = 0.5;
        float yaw;
        if (props.containsKey("rotation")) {
            yaw = Integer.parseInt(props.get("rotation")) * 22.5f;
        } else {
            final String facing = props.getOrDefault("facing", "south");
            yaw = switch (facing) {
                case "west" -> 90f;
                case "north" -> 180f;
                case "east" -> 270f;
                default -> 0f;
            };
            if (name.endsWith("_wall_sign") && !name.endsWith("_wall_hanging_sign")) {
                final double[] step = switch (facing) {
                    case "north" -> new double[] {0, -1};
                    case "south" -> new double[] {0, 1};
                    case "west" -> new double[] {-1, 0};
                    default -> new double[] {1, 0};
                };
                cx -= step[0] * 0.4375;
                cz -= step[1] * 0.4375;
            }
        }
        final double dx = x - (this.getX() + cx);
        final double dz = z - (this.getZ() + cz);
        final float toPoint = (float) Math.toDegrees(Math.atan2(dz, dx)) - 90f;
        float diff = (toPoint - yaw) % 360f;
        if (diff >= 180f) {
            diff -= 360f;
        } else if (diff < -180f) {
            diff += 360f;
        }
        return Math.abs(diff) <= 90f ? org.bukkit.block.sign.Side.FRONT : org.bukkit.block.sign.Side.BACK;
    }

    public boolean isWaxed() {
        return Values.bool(this.sign(), "waxed", false);
    }

    public void setWaxed(final boolean waxed) {
        this.sign().put("waxed", waxed);
        this.signChanged = true;
    }

    public java.util.@Nullable UUID getAllowedEditorUniqueId() {
        return this.allowedEditor;
    }

    public void setAllowedEditorUniqueId(final java.util.@Nullable UUID uuid) {
        this.allowedEditor = uuid;
    }

    public org.bukkit.entity.@Nullable Player getAllowedEditor() {
        return this.allowedEditor == null ? null : org.bukkit.Bukkit.getPlayer(this.allowedEditor);
    }

    public boolean isOperatorFeaturesAllowed() {
        return false;
    }

    public void setOperatorFeaturesAllowed(final boolean allowed) {
    }

    public BlockState copy() {
        return HostProxies.create(new HostBlockState(this.block, this.iface, this.data), this.iface);
    }

    public BlockState copy(final Location location) {
        final HostWorld world = (HostWorld) (location.getWorld() == null ? this.block.getWorld() : location.getWorld());
        return HostProxies.create(new HostBlockState((HostBlock) world.getBlockAt(location), this.iface, this.data), this.iface);
    }

    private @Nullable Map<String, Object> container() {
        final HostWorld world = (HostWorld) this.block.getWorld();
        final Object info = world.server().call("inventory.of_block", Values.of("world", world.getName(), "x", this.getX(), "y", this.getY(), "z", this.getZ()));
        return info instanceof Map<?, ?> m ? Values.map(m) : null;
    }

    /** The container's inventory (both halves for a double chest). */
    public Inventory getInventory() {
        final Map<String, Object> info = this.container();
        if (info == null) {
            throw new IllegalStateException("The block at " + this.getLocation() + " is no longer a container");
        }
        return Inventories.block(((HostWorld) this.block.getWorld()).server(), Values.map(info.get("inventory")),
            Inventories.type(Values.str(info, "type", "CHEST")), Values.i32(info, "size", 27));
    }

    public Inventory getSnapshotInventory() {
        return this.getBlockInventory();
    }

    /** This block's own slots (one half of a double chest). */
    public Inventory getBlockInventory() {
        final HostWorld world = (HostWorld) this.block.getWorld();
        final InventoryType type = switch (this.type.getKey().getKey()) {
            case "barrel" -> InventoryType.BARREL;
            case "furnace" -> InventoryType.FURNACE;
            case "smoker" -> InventoryType.SMOKER;
            case "blast_furnace" -> InventoryType.BLAST_FURNACE;
            case "hopper" -> InventoryType.HOPPER;
            case "dispenser" -> InventoryType.DISPENSER;
            case "dropper" -> InventoryType.DROPPER;
            default -> InventoryType.CHEST;
        };
        return Inventories.block(world.server(), Values.of("kind", "block", "world", world.getName(), "x", this.getX(), "y", this.getY(), "z", this.getZ()),
            type, type.getDefaultSize());
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostBlockState other && other.block.equals(this.block);
    }

    @Override
    public int hashCode() {
        return this.block.hashCode();
    }

    @Override
    public String toString() {
        return "CraftBlockState{" + this.type + " at " + this.block + "}";
    }
}
