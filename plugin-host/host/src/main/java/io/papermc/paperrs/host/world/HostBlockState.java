package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.Components;

import io.papermc.paperrs.host.HostProxies;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.inventory.Inventories;
import io.papermc.paperrs.host.item.HostPersistentDataContainer;
import io.papermc.paperrs.host.item.Nbt;
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
import org.bukkit.block.TileState;
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
        final Map<String, Object> info = block.info();
        return of(block, Values.str(info, "state", "minecraft:air"), Values.bool(info, "block_entity", false));
    }

    /** A snapshot of {@code block} holding {@code state} (for events about a state the world no longer has). */
    public static BlockState of(final HostBlock block, final String state) {
        return of(block, state, false);
    }

    /** A snapshot; blocks with a block entity of no more particular kind are {@link TileState}s. */
    private static BlockState of(final HostBlock block, final String state, final boolean blockEntity) {
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
            case "spawner" -> org.bukkit.block.CreatureSpawner.class;
            default -> path.endsWith("_banner") ? org.bukkit.block.Banner.class
                : path.endsWith("shulker_box") ? org.bukkit.block.ShulkerBox.class
                : blockEntity ? TileState.class : BlockState.class;
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
        this.writeTile();
        if (this.signChanged && this.sign != null) {
            final HostWorld world = (HostWorld) this.block.getWorld();
            final Map<String, Object> args = new java.util.HashMap<>(this.sign);
            args.putAll(Values.of("world", world.getName(), "x", this.getX(), "y", this.getY(), "z", this.getZ()));
            world.server().call("world.set_sign", args);
            this.signChanged = false;
        }
        return true;
    }

    // --- metadata (kept by the block, as CraftBlockState does) --------------------------------

    public void setMetadata(final String key, final org.bukkit.metadata.MetadataValue value) {
        this.block.setMetadata(key, value);
    }

    public List<org.bukkit.metadata.MetadataValue> getMetadata(final String key) {
        return this.block.getMetadata(key);
    }

    public boolean hasMetadata(final String key) {
        return this.block.hasMetadata(key);
    }

    public void removeMetadata(final String key, final org.bukkit.plugin.Plugin plugin) {
        this.block.removeMetadata(key, plugin);
    }

    public org.bukkit.Chunk getChunk() {
        return this.block.getChunk();
    }

    // --- block entities (CraftBlockEntityState: a snapshot, written back by update) ----------

    /** The block entity as read when first asked: {nbt, name}; empty where there is none. */
    private @Nullable Map<String, Object> tile;
    private Nbt.@Nullable Compound tileNbt;
    /** Tags the snapshot changed, written back over the block entity's by update. */
    private final Nbt.Compound tileSet = new Nbt.Compound();
    private final java.util.Set<String> tileRemoved = new java.util.HashSet<>();
    private net.kyori.adventure.text.@Nullable Component customName;
    private boolean nameChanged;
    private @Nullable HostPersistentDataContainer tilePdc;
    private boolean tilePdcChanged;

    private Map<String, Object> tile() {
        if (this.tile == null) {
            final HostWorld world = (HostWorld) this.block.getWorld();
            final Object read = world.server().call("world.tile_data", Values.of("world", world.getName(), "x", this.getX(), "y", this.getY(), "z", this.getZ()));
            this.tile = read instanceof Map<?, ?> m ? Values.map(m) : new java.util.HashMap<>();
        }
        return this.tile;
    }

    /** The block entity's tags as read (an empty compound where there is none). */
    private Nbt.Compound tileNbt() {
        if (this.tileNbt == null) {
            this.tileNbt = this.tile().get("nbt") instanceof byte[] bytes && bytes.length > 0 ? Nbt.readCompound(bytes) : new Nbt.Compound();
        }
        return this.tileNbt;
    }

    /** A tag of the snapshot: as changed, or as read. */
    private @Nullable Object tileTag(final String key) {
        if (this.tileRemoved.contains(key)) {
            return null;
        }
        final Object changed = this.tileSet.get(key);
        return changed != null ? changed : this.tileNbt().get(key);
    }

    private void setTileTag(final String key, final @Nullable Object value) {
        if (value == null) {
            this.tileSet.remove(key);
            this.tileRemoved.add(key);
        } else {
            this.tileRemoved.remove(key);
            this.tileSet.put(key, value);
        }
    }

    private void writeTile() {
        if (this.tilePdcChanged && this.tilePdc != null) {
            this.setTileTag(HostPersistentDataContainer.BUKKIT_KEY, this.tilePdc.isEmpty() ? null : this.tilePdc.tags().copy());
            this.tilePdcChanged = false;
        }
        if (this.tileSet.isEmpty() && this.tileRemoved.isEmpty() && !this.nameChanged) {
            return;
        }
        final HostWorld world = (HostWorld) this.block.getWorld();
        world.server().call("world.set_tile_data", Values.of("world", world.getName(), "x", this.getX(), "y", this.getY(), "z", this.getZ(),
            "set", this.tileSet.isEmpty() ? new byte[0] : Nbt.writeCompound(this.tileSet), "remove", new java.util.ArrayList<>(this.tileRemoved),
            "name_changed", this.nameChanged, "name", Components.toJson(this.customName)));
    }

    /** The block entity's custom name (containers, banners, beacons...: {@code Nameable}). */
    public net.kyori.adventure.text.@Nullable Component customName() {
        return this.nameChanged ? this.customName : Components.fromJson(this.tile().get("name"));
    }

    public void customName(final net.kyori.adventure.text.@Nullable Component name) {
        this.customName = name;
        this.nameChanged = true;
    }

    public @Nullable String getCustomName() {
        final net.kyori.adventure.text.Component name = this.customName();
        return name == null ? null : net.kyori.adventure.text.serializer.legacy.LegacyComponentSerializer.legacySection().serialize(name);
    }

    public void setCustomName(final @Nullable String name) {
        this.customName(name == null ? null : net.kyori.adventure.text.serializer.legacy.LegacyComponentSerializer.legacySection().deserialize(name));
    }

    /** What plugins keep on the block entity ({@code PublicBukkitValues}), saved by update. */
    public org.bukkit.persistence.PersistentDataContainer getPersistentDataContainer() {
        if (this.tilePdc == null) {
            final Object saved = this.tileNbt().get(HostPersistentDataContainer.BUKKIT_KEY);
            this.tilePdc = (saved instanceof Nbt.Compound c ? new HostPersistentDataContainer(c.copy()) : new HostPersistentDataContainer()).onChange(() -> this.tilePdcChanged = true);
        }
        return this.tilePdc;
    }

    public boolean isSnapshot() {
        return true;
    }

    // --- banners (CraftBanner) -----------------------------------------------------------------

    private java.util.@Nullable List<org.bukkit.block.banner.Pattern> patterns;
    private org.bukkit.@Nullable DyeColor baseColor;

    public org.bukkit.DyeColor getBaseColor() {
        if (this.baseColor == null) {
            final String path = this.type.getKey().getKey();
            final String color = path.substring(0, path.indexOf(path.contains("_wall_banner") ? "_wall_banner" : "_banner"));
            this.baseColor = org.bukkit.DyeColor.valueOf(color.toUpperCase(java.util.Locale.ROOT));
        }
        return this.baseColor;
    }

    /** As on Paper, kept by the snapshot only: a placed banner's colour is its block's. */
    public void setBaseColor(final org.bukkit.DyeColor color) {
        if (color == null) {
            throw new IllegalArgumentException("color");
        }
        this.baseColor = color;
    }

    private java.util.List<org.bukkit.block.banner.Pattern> patterns() {
        if (this.patterns == null) {
            this.patterns = new java.util.ArrayList<>();
            if (this.tileTag("patterns") instanceof Nbt.NbtList list) {
                for (final Object entry : list) {
                    if (entry instanceof Nbt.Compound layer && layer.get("pattern") instanceof String id && layer.get("color") instanceof String color) {
                        final org.bukkit.NamespacedKey key = org.bukkit.NamespacedKey.fromString(id);
                        final org.bukkit.block.banner.PatternType type = key == null ? null : org.bukkit.Registry.BANNER_PATTERN.get(key);
                        if (type != null) {
                            this.patterns.add(new org.bukkit.block.banner.Pattern(org.bukkit.DyeColor.valueOf(color.toUpperCase(java.util.Locale.ROOT)), type));
                        }
                    }
                }
            }
        }
        return this.patterns;
    }

    private void patternsChanged() {
        final Nbt.NbtList list = new Nbt.NbtList(Nbt.COMPOUND);
        for (final org.bukkit.block.banner.Pattern pattern : this.patterns()) {
            final Nbt.Compound layer = new Nbt.Compound();
            layer.put("color", pattern.getColor().name().toLowerCase(java.util.Locale.ROOT));
            layer.put("pattern", pattern.getPattern().getKey().asString());
            list.add(layer);
        }
        this.setTileTag("patterns", list.isEmpty() ? null : list);
    }

    public java.util.List<org.bukkit.block.banner.Pattern> getPatterns() {
        return new java.util.ArrayList<>(this.patterns());
    }

    public void setPatterns(final java.util.List<org.bukkit.block.banner.Pattern> patterns) {
        this.patterns = new java.util.ArrayList<>(patterns);
        this.patternsChanged();
    }

    public void addPattern(final org.bukkit.block.banner.Pattern pattern) {
        this.patterns().add(pattern);
        this.patternsChanged();
    }

    public org.bukkit.block.banner.Pattern getPattern(final int i) {
        return this.patterns().get(i);
    }

    public org.bukkit.block.banner.Pattern removePattern(final int i) {
        final org.bukkit.block.banner.Pattern removed = this.patterns().remove(i);
        this.patternsChanged();
        return removed;
    }

    public void setPattern(final int i, final org.bukkit.block.banner.Pattern pattern) {
        this.patterns().set(i, pattern);
        this.patternsChanged();
    }

    public int numberOfPatterns() {
        return this.patterns().size();
    }

    // --- spawners (CraftCreatureSpawner): the block entity's tags -----------------------------

    public org.bukkit.entity.@Nullable EntityType getSpawnedType() {
        if (this.tileTag("SpawnData") instanceof Nbt.Compound data && data.get("entity") instanceof Nbt.Compound entity && entity.get("id") instanceof String id) {
            final org.bukkit.NamespacedKey key = org.bukkit.NamespacedKey.fromString(id);
            return key == null ? null : org.bukkit.Registry.ENTITY_TYPE.get(key);
        }
        return null;
    }

    public void setSpawnedType(final org.bukkit.entity.@Nullable EntityType type) {
        if (type == null) {
            this.setTileTag("SpawnData", null);
            this.setTileTag("SpawnPotentials", null);
            return;
        }
        if (!type.isSpawnable()) {
            throw new IllegalArgumentException("Can't spawn EntityType " + type + " from mob spawners!");
        }
        final Nbt.Compound entity = new Nbt.Compound();
        entity.put("id", type.getKey().asString());
        final Nbt.Compound data = new Nbt.Compound();
        data.put("entity", entity);
        this.setTileTag("SpawnData", data);
        this.setTileTag("SpawnPotentials", null);
    }

    private int spawnerValue(final String key, final int fallback) {
        return this.tileTag(key) instanceof Number n ? n.intValue() : fallback;
    }

    public int getDelay() {
        return this.spawnerValue("Delay", 20);
    }

    public void setDelay(final int delay) {
        this.setTileTag("Delay", (short) delay);
    }

    public int getMinSpawnDelay() {
        return this.spawnerValue("MinSpawnDelay", 200);
    }

    public void setMinSpawnDelay(final int delay) {
        if (delay > this.getMaxSpawnDelay()) {
            throw new IllegalArgumentException("Minimum Spawn Delay must be less than or equal to Maximum Spawn Delay");
        }
        this.setTileTag("MinSpawnDelay", (short) delay);
    }

    public int getMaxSpawnDelay() {
        return this.spawnerValue("MaxSpawnDelay", 800);
    }

    public void setMaxSpawnDelay(final int delay) {
        if (delay <= 0) {
            throw new IllegalArgumentException("Maximum Spawn Delay must be greater than 0.");
        }
        if (delay < this.getMinSpawnDelay()) {
            throw new IllegalArgumentException("Maximum Spawn Delay must be greater than or equal to Minimum Spawn Delay");
        }
        this.setTileTag("MaxSpawnDelay", (short) delay);
    }

    public int getSpawnCount() {
        return this.spawnerValue("SpawnCount", 4);
    }

    public void setSpawnCount(final int count) {
        if (count < 0) {
            throw new IllegalArgumentException("Spawn count must be greater than or equal to 0");
        }
        this.setTileTag("SpawnCount", (short) count);
    }

    public int getMaxNearbyEntities() {
        return this.spawnerValue("MaxNearbyEntities", 6);
    }

    public void setMaxNearbyEntities(final int max) {
        this.setTileTag("MaxNearbyEntities", (short) max);
    }

    public int getRequiredPlayerRange() {
        return this.spawnerValue("RequiredPlayerRange", 16);
    }

    public void setRequiredPlayerRange(final int range) {
        this.setTileTag("RequiredPlayerRange", (short) range);
    }

    public int getSpawnRange() {
        return this.spawnerValue("SpawnRange", 4);
    }

    public void setSpawnRange(final int range) {
        this.setTileTag("SpawnRange", (short) range);
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
