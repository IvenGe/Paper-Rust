package io.papermc.paperrs.test.bukkit;

import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import java.util.Map;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import org.bukkit.Bukkit;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.OfflinePlayer;
import org.bukkit.World;
import org.bukkit.block.Block;
import org.bukkit.block.BlockState;
import org.bukkit.block.Chest;
import org.bukkit.command.Command;
import org.bukkit.command.CommandExecutor;
import org.bukkit.command.CommandSender;
import org.bukkit.enchantments.Enchantment;
import org.bukkit.entity.Entity;
import org.bukkit.entity.Item;
import org.bukkit.entity.LivingEntity;
import org.bukkit.entity.Player;
import org.bukkit.entity.Zombie;
import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.event.entity.CreatureSpawnEvent;
import org.bukkit.event.entity.EntityDamageByEntityEvent;
import org.bukkit.event.entity.EntityDeathEvent;
import org.bukkit.event.entity.EntityPickupItemEvent;
import org.bukkit.event.entity.PlayerDeathEvent;
import org.bukkit.event.inventory.InventoryClickEvent;
import org.bukkit.event.inventory.InventoryCloseEvent;
import org.bukkit.event.inventory.InventoryOpenEvent;
import org.bukkit.event.player.PlayerDropItemEvent;
import org.bukkit.event.player.PlayerRespawnEvent;
import org.bukkit.event.server.ServerLoadEvent;
import org.bukkit.inventory.Inventory;
import org.bukkit.inventory.InventoryHolder;
import org.bukkit.inventory.ItemFlag;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.PlayerInventory;
import org.bukkit.inventory.meta.Damageable;
import org.bukkit.inventory.meta.ItemMeta;
import org.bukkit.persistence.PersistentDataType;
import org.bukkit.plugin.java.JavaPlugin;
import org.bukkit.scoreboard.Criteria;
import org.bukkit.scoreboard.DisplaySlot;
import org.bukkit.scoreboard.Objective;
import org.bukkit.scoreboard.Scoreboard;
import org.bukkit.scoreboard.Team;

/**
 * Exercises the plugin API areas end to end for {@code tests/java_plugins.rs}: item stacks
 * and metas, inventories and inventory events, entities, offline players, scoreboards and
 * {@code /reload}. Results are reported to the player (or the log) as plain text lines.
 */
final class ApiTests implements Listener, CommandExecutor {
    private final JavaPlugin plugin;
    private final NamespacedKey ownerKey;

    /** Jumps the server counted, as PlayerStatisticIncrementEvent reported them (and cancelled). */
    private final List<String> jumps = new ArrayList<>();

    ApiTests(final JavaPlugin plugin) {
        this.plugin = plugin;
        this.ownerKey = new NamespacedKey(plugin, "owner");
    }

    /** The holder that marks the test menu, as GUI plugins do. */
    static final class TestMenu implements InventoryHolder {
        private Inventory inventory;

        @Override
        public Inventory getInventory() {
            return this.inventory;
        }
    }

    private static void check(final List<String> failures, final boolean ok, final String what) {
        if (!ok) {
            failures.add(what);
        }
    }

    private static void report(final CommandSender sender, final String area, final List<String> failures) {
        sender.sendMessage(area + ": " + (failures.isEmpty() ? "ok" : "FAIL " + String.join(", ", failures)));
    }

    @EventHandler
    public void onStatistic(final org.bukkit.event.player.PlayerStatisticIncrementEvent event) {
        if (event.getStatistic() == org.bukkit.Statistic.JUMP) {
            this.jumps.add(event.getStatistic() + " " + event.getPreviousValue() + "->" + event.getNewValue());
            event.setCancelled(true);
        }
    }

    @Override
    public boolean onCommand(final CommandSender sender, final Command command, final String label, final String[] args) {
        if (args.length == 0) {
            return false;
        }
        try {
            switch (args[0]) {
                case "items" -> this.items((Player) sender);
                case "gui" -> this.gui((Player) sender);
                case "chest" -> this.chest(sender, ((Player) sender).getWorld().getBlockAt(Integer.parseInt(args[1]), Integer.parseInt(args[2]), Integer.parseInt(args[3])));
                case "entities" -> this.entities((Player) sender);
                case "offline" -> this.offline(sender, args[1]);
                case "scoreboard" -> this.scoreboard((Player) sender);
                case "effects" -> this.effects((Player) sender);
                case "bossbar" -> this.bossbar((Player) sender);
                case "blockdata" -> this.blockData((Player) sender);
                case "metas" -> this.metas((Player) sender);
                case "attributes" -> this.attributes((Player) sender);
                case "packs" -> {
                    ((Player) sender).setResourcePack(java.util.UUID.fromString("5d1ac9b3-08a1-4b7e-9a51-4c6a8e44f1c2"), "https://example.com/pack.zip", new byte[20],
                        Component.text("Please accept"), true);
                    sender.sendMessage("packs: sent");
                }
                case "packstatus" -> sender.sendMessage("packstatus: " + ((Player) sender).getResourcePackStatus() + " " + String.join(",", this.packStatuses));
                case "book" -> {
                    ((Player) sender).openBook(net.kyori.adventure.inventory.Book.builder().title(Component.text("Guide")).author(Component.text("Alice")).addPage(Component.text("Hello")).build());
                    sender.sendMessage("book: opened");
                }
                case "villagers" -> this.villagers((Player) sender);
                case "legacy" -> this.legacy(sender);
                case "playerstate" -> this.playerState((Player) sender);
                case "worldstate" -> this.worldState((Player) sender);
                case "signs" -> this.signs((Player) sender);
                case "worldapi" -> this.worldApi((Player) sender);
                case "merchant" -> this.merchant((Player) sender);
                case "career" -> this.career((Player) sender);
                case "riding" -> this.riding((Player) sender);
                case "pdc" -> {
                    final Player player = (Player) sender;
                    final org.bukkit.NamespacedKey key = new org.bukkit.NamespacedKey(this.plugin, "score");
                    player.getPersistentDataContainer().set(key, org.bukkit.persistence.PersistentDataType.INTEGER, 42);
                    final org.bukkit.entity.Pig pig = player.getWorld().spawn(player.getLocation().add(2, 0, 0), org.bukkit.entity.Pig.class);
                    pig.setCustomName("Kept");
                    pig.getPersistentDataContainer().set(key, org.bukkit.persistence.PersistentDataType.STRING, "oink");
                    sender.sendMessage("pdc: " + player.getPersistentDataContainer().get(key, org.bukkit.persistence.PersistentDataType.INTEGER) + " "
                        + pig.getPersistentDataContainer().get(key, org.bukkit.persistence.PersistentDataType.STRING));
                }
                case "attrmods" -> {
                    final ItemStack sword = new ItemStack(Material.DIAMOND_SWORD);
                    final org.bukkit.inventory.meta.ItemMeta meta = sword.getItemMeta();
                    final org.bukkit.NamespacedKey key = new org.bukkit.NamespacedKey(this.plugin, "bonus");
                    meta.addAttributeModifier(org.bukkit.attribute.Attribute.ATTACK_DAMAGE,
                        new org.bukkit.attribute.AttributeModifier(key, 10.0, org.bukkit.attribute.AttributeModifier.Operation.ADD_NUMBER, org.bukkit.inventory.EquipmentSlotGroup.MAINHAND));
                    sword.setItemMeta(meta);
                    final org.bukkit.inventory.meta.ItemMeta back = sword.getItemMeta();
                    final org.bukkit.attribute.AttributeModifier modifier = back.getAttributeModifiers(org.bukkit.attribute.Attribute.ATTACK_DAMAGE).iterator().next();
                    ((Player) sender).getInventory().setItemInMainHand(sword);
                    sender.sendMessage("attrmods: " + back.hasAttributeModifiers() + " " + modifier.getKey().getKey() + " " + modifier.getAmount() + " " + modifier.getSlotGroup() + " "
                        + back.getAttributeModifiers(org.bukkit.inventory.EquipmentSlot.HAND).size() + " " + back.getAttributeModifiers(org.bukkit.inventory.EquipmentSlot.HEAD).size());
                }
                case "stats" -> {
                    final Player player = (Player) sender;
                    final List<String> failures = new ArrayList<>();
                    player.setStatistic(org.bukkit.Statistic.JUMP, 5);
                    player.incrementStatistic(org.bukkit.Statistic.JUMP, 3);
                    player.decrementStatistic(org.bukkit.Statistic.JUMP);
                    check(failures, player.getStatistic(org.bukkit.Statistic.JUMP) == 7, "jumps " + player.getStatistic(org.bukkit.Statistic.JUMP));
                    player.incrementStatistic(org.bukkit.Statistic.MINE_BLOCK, Material.STONE, 4);
                    check(failures, player.getStatistic(org.bukkit.Statistic.MINE_BLOCK, Material.STONE) == 4, "stone mined");
                    player.setStatistic(org.bukkit.Statistic.KILL_ENTITY, org.bukkit.entity.EntityType.ZOMBIE, 2);
                    check(failures, player.getStatistic(org.bukkit.Statistic.KILL_ENTITY, org.bukkit.entity.EntityType.ZOMBIE) == 2, "zombies killed");
                    player.incrementStatistic(org.bukkit.Statistic.CHEST_OPENED);
                    check(failures, player.getStatistic(org.bukkit.Statistic.CHEST_OPENED) == 1, "chests opened");
                    // Offline players read the same saved counts.
                    check(failures, Bukkit.getOfflinePlayer(player.getUniqueId()).getStatistic(org.bukkit.Statistic.JUMP) == 7, "offline jumps");
                    // CraftStatistic's argument checks.
                    for (final Runnable bad : List.<Runnable>of(
                        () -> player.getStatistic(org.bukkit.Statistic.MINE_BLOCK),
                        () -> player.getStatistic(org.bukkit.Statistic.MINE_BLOCK, Material.DIAMOND),
                        () -> player.getStatistic(org.bukkit.Statistic.JUMP, org.bukkit.entity.EntityType.ZOMBIE),
                        () -> player.incrementStatistic(org.bukkit.Statistic.JUMP, 0),
                        () -> player.decrementStatistic(org.bukkit.Statistic.DEATHS, 1000))) {
                        try {
                            bad.run();
                            failures.add("accepted a bad argument");
                        } catch (IllegalArgumentException expected) {
                            // As CraftStatistic.
                        }
                    }
                    report(sender, "stats", failures);
                }
                case "owner" -> {
                    // A stick only someone else may pick up, and a feather for this player.
                    final Player player = (Player) sender;
                    final java.util.UUID stranger = java.util.UUID.randomUUID();
                    final org.bukkit.entity.Item stick = player.getWorld().dropItem(player.getLocation(), new ItemStack(Material.STICK));
                    stick.setOwner(stranger);
                    stick.setThrower(stranger);
                    final org.bukkit.entity.Item feather = player.getWorld().dropItem(player.getLocation(), new ItemStack(Material.FEATHER));
                    feather.setOwner(player.getUniqueId());
                    sender.sendMessage("owner: " + stranger.equals(stick.getOwner()) + " " + stranger.equals(stick.getThrower()) + " " + player.getUniqueId().equals(feather.getOwner()) + " " + (feather.getThrower() == null));
                }
                case "chestboat" -> {
                    // A chest boat the plugin spawns: its chest takes items and knows its holder.
                    final Player player = (Player) sender;
                    final List<String> failures = new ArrayList<>();
                    final org.bukkit.entity.Entity spawned = player.getWorld().spawnEntity(player.getLocation().add(3, 0, 0), org.bukkit.entity.EntityType.OAK_CHEST_BOAT);
                    if (spawned instanceof org.bukkit.entity.ChestBoat boat) {
                        final org.bukkit.inventory.Inventory chest = boat.getInventory();
                        check(failures, chest.getSize() == 27, "27 slots");
                        check(failures, chest.getHolder() == boat, "held by the boat");
                        chest.setItem(4, new org.bukkit.inventory.ItemStack(org.bukkit.Material.DIAMOND, 3));
                        check(failures, boat.getInventory().contains(org.bukkit.Material.DIAMOND, 3), "the diamonds in the chest");
                    } else {
                        failures.add("not a chest boat: " + spawned.getType());
                    }
                    report(sender, "chestboat", failures);
                }
                case "worlds" -> {
                    // A flat world made by the plugin, once; Alice goes there.
                    final Player player = (Player) sender;
                    final List<String> failures = new ArrayList<>();
                    final org.bukkit.World flat = Bukkit.createWorld(new org.bukkit.WorldCreator("plugin_flat").type(org.bukkit.WorldType.FLAT));
                    check(failures, flat != null && "plugin_flat".equals(flat.getName()) && flat.getEnvironment() == org.bukkit.World.Environment.NORMAL, "created");
                    check(failures, Bukkit.getWorld("plugin_flat") == flat, "listed");
                    check(failures, Bukkit.createWorld(new org.bukkit.WorldCreator("plugin_flat")) == flat, "made once");
                    try {
                        Bukkit.createWorld(new org.bukkit.WorldCreator("plugin_amplified").type(org.bukkit.WorldType.AMPLIFIED));
                        failures.add("an amplified world");
                    } catch (RuntimeException expected) {
                        // Not supported yet: the server says so.
                    }
                    if (flat != null) {
                        player.teleport(new org.bukkit.Location(flat, 8.5, -60, 8.5));
                    }
                    report(sender, "worlds", failures);
                }
                case "statevents" -> sender.sendMessage("statevents: " + String.join(", ", this.jumps) + " now " + ((Player) sender).getStatistic(org.bukkit.Statistic.JUMP));
                case "unsupported" -> {
                    try {
                        ((Player) sender).getWorld().locateNearestRaid(((Player) sender).getLocation(), 16);
                        sender.sendMessage("unsupported: no exception");
                    } catch (UnsupportedOperationException e) {
                        sender.sendMessage("unsupported: " + e.getMessage());
                    }
                }
                default -> {
                    return false;
                }
            }
        } catch (RuntimeException e) {
            this.plugin.getLogger().log(java.util.logging.Level.SEVERE, "apitest " + args[0] + " failed", e);
            sender.sendMessage(args[0] + ": FAIL " + e);
        }
        return true;
    }

    // --- villagers and merchants --------------------------------------------------------------

    /** A plugin's merchant stays while the plugin holds it. */
    private org.bukkit.inventory.Merchant shop;

    private static org.bukkit.inventory.MerchantRecipe recipe(final Material result, final int amount, final Material cost, final int costAmount, final int maxUses) {
        final org.bukkit.inventory.MerchantRecipe recipe = new org.bukkit.inventory.MerchantRecipe(new ItemStack(result, amount), 0, maxUses, true, 2, 0.05f);
        recipe.addIngredient(new ItemStack(cost, costAmount));
        return recipe;
    }

    /**
     * Commodore and legacy materials: as on Paper, a plugin with an api-version sees no
     * {@code LEGACY_} constants in {@code Material.values()} (nor in an enum switch), while the
     * legacy constants it does name still convert.
     */
    /** A player's sign text that starts with "shout" is shouted (SignChangeEvent). */
    @EventHandler
    public void onSignChange(final org.bukkit.event.block.SignChangeEvent event) {
        final String first = net.kyori.adventure.text.serializer.plain.PlainTextComponentSerializer.plainText().serialize(event.line(0));
        if (first.startsWith("shout")) {
            event.line(0, Component.text(first.toUpperCase(java.util.Locale.ROOT)));
        }
    }

    /** Sign block states: both sides, colour, glow and wax, written with update(). */
    private void signs(final Player player) {
        final List<String> failures = new ArrayList<>();
        final org.bukkit.block.Block block = player.getLocation().add(-3, 0, -3).getBlock();
        block.setType(Material.OAK_SIGN);
        final org.bukkit.block.Sign sign = (org.bukkit.block.Sign) block.getState();
        sign.getSide(org.bukkit.block.sign.Side.FRONT).line(0, Component.text("Hello"));
        sign.getSide(org.bukkit.block.sign.Side.BACK).setLine(1, "Back side");
        sign.getSide(org.bukkit.block.sign.Side.FRONT).setColor(org.bukkit.DyeColor.BLUE);
        sign.getSide(org.bukkit.block.sign.Side.FRONT).setGlowingText(true);
        sign.setWaxed(true);
        check(failures, sign.update(), "update");
        final org.bukkit.block.Sign again = (org.bukkit.block.Sign) block.getState();
        final net.kyori.adventure.text.serializer.plain.PlainTextComponentSerializer plain = net.kyori.adventure.text.serializer.plain.PlainTextComponentSerializer.plainText();
        check(failures, "Hello".equals(plain.serialize(again.getSide(org.bukkit.block.sign.Side.FRONT).line(0))), "front line " + again.getSide(org.bukkit.block.sign.Side.FRONT).line(0));
        check(failures, "Back side".equals(again.getSide(org.bukkit.block.sign.Side.BACK).getLine(1)), "back line " + again.getSide(org.bukkit.block.sign.Side.BACK).getLine(1));
        check(failures, again.getSide(org.bukkit.block.sign.Side.FRONT).getColor() == org.bukkit.DyeColor.BLUE && again.getSide(org.bukkit.block.sign.Side.FRONT).isGlowingText(), "colour and glow");
        check(failures, again.isWaxed() && "Hello".equals(again.getLine(0)), "waxed, old API");
        block.setType(Material.AIR);
        report(player, "signs", failures);
    }

    /** A plugin may change a game rule value on its way (WorldGameRuleChangeEvent). */
    @EventHandler
    public void onGameRuleChange(final io.papermc.paper.event.world.WorldGameRuleChangeEvent event) {
        if (event.getGameRule().equals(org.bukkit.GameRules.MAX_ENTITY_CRAMMING) && "13".equals(event.getValue())) {
            event.setValue("14");
        }
    }

    /**
     * Game rules, the world's own plugin data and metadata, blocks broken naturally, and block
     * entities: custom names, plugin data, banners and spawners.
     */
    private void worldApi(final Player player) {
        final List<String> failures = new ArrayList<>();
        final World world = player.getWorld();
        // Game rules: by rule and by name, a legacy constant, and a listener changing a value.
        check(failures, !world.getGameRuleValue(org.bukkit.GameRules.KEEP_INVENTORY), "keep_inventory off");
        check(failures, world.setGameRule(org.bukkit.GameRules.KEEP_INVENTORY, true) && world.getGameRuleValue(org.bukkit.GameRules.KEEP_INVENTORY), "keep_inventory on");
        check(failures, "true".equals(world.getGameRuleValue("keep_inventory")) && world.isGameRule("minecraft:keep_inventory") && !world.isGameRule("no_such_rule"), "rules by name");
        check(failures, world.setGameRule(org.bukkit.GameRules.RANDOM_TICK_SPEED, 5) && world.getGameRuleValue(org.bukkit.GameRules.RANDOM_TICK_SPEED) == 5, "random_tick_speed");
        @SuppressWarnings("deprecation") final org.bukkit.GameRule<Boolean> fireTick = org.bukkit.GameRule.DO_FIRE_TICK;
        check(failures, world.getGameRuleValue(fireTick) && world.setGameRule(fireTick, false) && world.getGameRuleValue(org.bukkit.GameRules.FIRE_SPREAD_RADIUS_AROUND_PLAYER) == 0, "fire tick, a legacy rule");
        world.setGameRule(fireTick, true);
        check(failures, world.setGameRule(org.bukkit.GameRules.MAX_ENTITY_CRAMMING, 13) && world.getGameRuleValue(org.bukkit.GameRules.MAX_ENTITY_CRAMMING) == 14, "changed by a listener");
        check(failures, java.util.Arrays.asList(world.getGameRules()).contains("minecraft:keep_inventory") && world.getPVP(), "rule names, pvp");
        // The world's own plugin data, and metadata on worlds and blocks.
        final NamespacedKey key = new NamespacedKey(this.plugin, "visits");
        world.getPersistentDataContainer().set(key, PersistentDataType.INTEGER, 42);
        check(failures, world.getPersistentDataContainer().get(key, PersistentDataType.INTEGER) == 42, "world data");
        final Block block = player.getLocation().add(3, 0, -3).getBlock();
        world.setMetadata("marker", new org.bukkit.metadata.FixedMetadataValue(this.plugin, "w"));
        block.setMetadata("marker", new org.bukkit.metadata.FixedMetadataValue(this.plugin, "b"));
        check(failures, "w".equals(world.getMetadata("marker").getFirst().asString()) && "b".equals(block.getMetadata("marker").getFirst().asString())
            && block.getState().hasMetadata("marker") && !block.getRelative(org.bukkit.block.BlockFace.UP).hasMetadata("marker"), "metadata");
        // Stone drops cobblestone with no tool or a pickaxe, nothing with a stick; breaking it
        // naturally leaves the cobblestone there.
        block.setType(Material.STONE);
        check(failures, block.getDrops().stream().anyMatch(i -> i.getType() == Material.COBBLESTONE), "drops of stone " + block.getDrops());
        check(failures, block.getDrops(new ItemStack(Material.STICK)).isEmpty() && !block.getDrops(new ItemStack(Material.WOODEN_PICKAXE)).isEmpty(), "drops by tool");
        check(failures, !block.isPassable() && block.getPistonMoveReaction() == org.bukkit.block.PistonMoveReaction.MOVE, "stone is solid and moves");
        check(failures, block.breakNaturally() && block.getType() == Material.AIR, "broken naturally");
        // A chest's name and plugin data, a banner's patterns, a spawner's mob.
        final Block chestBlock = player.getLocation().add(3, 0, -5).getBlock();
        chestBlock.setType(Material.CHEST);
        final Chest chest = (Chest) chestBlock.getState();
        chest.customName(Component.text("Loot"));
        chest.getPersistentDataContainer().set(key, PersistentDataType.STRING, "owned");
        check(failures, chest.update(), "chest update");
        final Chest chestAgain = (Chest) chestBlock.getState();
        check(failures, "Loot".equals(chestAgain.getCustomName()) && "owned".equals(chestAgain.getPersistentDataContainer().get(key, PersistentDataType.STRING)), "chest name and data " + chestAgain.getCustomName());
        final Block bannerBlock = player.getLocation().add(5, 0, -5).getBlock();
        bannerBlock.setType(Material.RED_BANNER);
        final org.bukkit.block.Banner banner = (org.bukkit.block.Banner) bannerBlock.getState();
        banner.addPattern(new org.bukkit.block.banner.Pattern(org.bukkit.DyeColor.WHITE, org.bukkit.block.banner.PatternType.STRIPE_TOP));
        check(failures, banner.update(), "banner update");
        final org.bukkit.block.Banner bannerAgain = (org.bukkit.block.Banner) bannerBlock.getState();
        check(failures, bannerAgain.getBaseColor() == org.bukkit.DyeColor.RED && bannerAgain.numberOfPatterns() == 1 && bannerAgain.getPattern(0).getColor() == org.bukkit.DyeColor.WHITE
            && bannerAgain.getPattern(0).getPattern() == org.bukkit.block.banner.PatternType.STRIPE_TOP, "banner patterns");
        final Block spawnerBlock = player.getLocation().add(5, 0, -7).getBlock();
        spawnerBlock.setType(Material.SPAWNER);
        final org.bukkit.block.CreatureSpawner spawner = (org.bukkit.block.CreatureSpawner) spawnerBlock.getState();
        spawner.setSpawnedType(org.bukkit.entity.EntityType.ZOMBIE);
        spawner.setDelay(100);
        check(failures, spawner.update(), "spawner update");
        final org.bukkit.block.CreatureSpawner spawnerAgain = (org.bukkit.block.CreatureSpawner) spawnerBlock.getState();
        check(failures, spawnerAgain.getSpawnedType() == org.bukkit.entity.EntityType.ZOMBIE && spawnerAgain.getDelay() == 100, "spawner " + spawnerAgain.getSpawnedType());
        check(failures, world.getWorldFolder().getName().equals(world.getName()), "world folder " + world.getWorldFolder());
        world.save();
        report(player, "worldapi", failures);
    }

    /** The world border and the weather of a world. */
    private void worldState(final Player player) {
        final List<String> failures = new ArrayList<>();
        final org.bukkit.World world = player.getWorld();
        final org.bukkit.WorldBorder border = world.getWorldBorder();
        check(failures, border.getSize() == border.getMaxSize() && border.isInside(player.getLocation()), "default border " + border.getSize());
        border.setCenter(100, 50);
        border.setSize(200);
        check(failures, border.getSize() == 200 && border.getCenter().getX() == 100 && border.getCenter().getZ() == 50, "border " + border.getSize() + " at " + border.getCenter());
        check(failures, border.isInside(new org.bukkit.Location(world, 150, 64, 100)) && !border.isInside(new org.bukkit.Location(world, 250, 64, 50)), "inside the border");
        border.setDamageAmount(1.5);
        border.setWarningDistance(7);
        check(failures, border.getDamageAmount() == 1.5 && border.getWarningDistance() == 7, "border damage and warning");
        border.changeSize(100, 400);
        check(failures, border.getSize() <= 200 && border.getSize() > 100, "border moving " + border.getSize());
        border.reset();
        check(failures, border.getSize() == border.getMaxSize() && border.getCenter().getX() == 0, "border reset");
        world.setStorm(true);
        world.setThundering(true);
        check(failures, world.hasStorm() && world.isThundering() && !world.isClearWeather(), "storm");
        world.setWeatherDuration(1200);
        check(failures, world.getWeatherDuration() == 1200, "weather duration " + world.getWeatherDuration());
        world.setStorm(false);
        world.setThundering(false);
        check(failures, world.isClearWeather(), "clear");
        // Chunks: loading a far chunk now (as getChunkAt does on Paper), forced chunks, tickets.
        check(failures, !world.isChunkGenerated(500, 500) && !world.isChunkLoaded(40, 40), "far chunks not there yet");
        final org.bukkit.Chunk far = world.getChunkAt(40, 40);
        check(failures, far.isLoaded() && far.isGenerated() && world.isChunkLoaded(40, 40), "far chunk loaded");
        check(failures, far.getBlock(0, -61, 0).getType() == Material.GRASS_BLOCK && far.getBlock(3, -64, 3).getType() == Material.BEDROCK, "far chunk's blocks " + far.getBlock(0, -61, 0).getType());
        check(failures, java.util.Arrays.asList(world.getLoadedChunks()).contains(far) && player.getLocation().getChunk().isLoaded(), "loaded chunks");
        world.setChunkForceLoaded(41, 41, true);
        check(failures, world.isChunkForceLoaded(41, 41) && world.getForceLoadedChunks().contains(world.getChunkAt(41, 41, false)), "forced chunk");
        world.setChunkForceLoaded(41, 41, false);
        check(failures, !world.isChunkForceLoaded(41, 41), "forced chunk released");
        check(failures, world.addPluginChunkTicket(42, 42, this.plugin) && !world.addPluginChunkTicket(42, 42, this.plugin) && world.getPluginChunkTickets(42, 42).contains(this.plugin), "plugin ticket");
        check(failures, world.removePluginChunkTicket(42, 42, this.plugin) && world.getPluginChunkTickets(42, 42).isEmpty(), "plugin ticket removed");
        report(player, "worldstate", failures);
    }

    /** Bukkit's player and entity setters that are plain state, read back. */
    @SuppressWarnings("deprecation")
    private void playerState(final Player player) {
        final List<String> failures = new ArrayList<>();
        player.setWalkSpeed(0.4f);
        player.setFlySpeed(0.3f);
        check(failures, Math.abs(player.getWalkSpeed() - 0.4f) < 1e-6 && Math.abs(player.getFlySpeed() - 0.3f) < 1e-6, "speeds " + player.getWalkSpeed() + " " + player.getFlySpeed());
        try {
            player.setWalkSpeed(2f);
            failures.add("walk speed 2 accepted");
        } catch (IllegalArgumentException expected) {
            // "Speed value (2.0) need to be between -1f and 1f"
        }
        player.setPlayerTime(6000, false);
        check(failures, player.getPlayerTimeOffset() == 6000 && !player.isPlayerTimeRelative() && player.getPlayerTime() % 24000 == 6000, "player time " + player.getPlayerTime());
        player.setPlayerWeather(org.bukkit.WeatherType.DOWNFALL);
        check(failures, player.getPlayerWeather() == org.bukkit.WeatherType.DOWNFALL, "player weather");
        player.setInvulnerable(true);
        final double health = player.getHealth();
        player.damage(4);
        check(failures, player.isInvulnerable() && player.getHealth() == health, "invulnerable " + player.getHealth());
        player.setInvulnerable(false);
        player.setGlowing(true);
        player.setSilent(true);
        player.setGravity(false);
        player.setCustomName("Champion");
        check(failures, player.isGlowing() && player.isSilent() && !player.hasGravity() && "Champion".equals(player.getCustomName()), "flags and name");
        player.setGlowing(false);
        player.setSilent(false);
        player.setGravity(true);
        player.setCustomName(null);
        check(failures, player.addScoreboardTag("vip") && !player.addScoreboardTag("vip") && player.getScoreboardTags().contains("vip"), "tags " + player.getScoreboardTags());
        check(failures, player.removeScoreboardTag("vip") && player.getScoreboardTags().isEmpty(), "tag removed");
        player.setFireTicks(100);
        player.setFallDistance(5f);
        player.setMaximumAir(400);
        player.setRemainingAir(350);
        player.setFreezeTicks(10);
        player.setExhaustion(1.5f);
        check(failures, player.getFireTicks() == 100 && player.getFallDistance() == 5f && player.getMaximumAir() == 400 && player.getRemainingAir() == 350
            && player.getFreezeTicks() == 10 && player.getExhaustion() == 1.5f, "air, fire, falling " + player.getFireTicks() + " " + player.getRemainingAir());
        player.setFireTicks(0);
        player.setFreezeTicks(0);
        player.setFallDistance(0f);
        player.setLevel(5);
        player.setExp(0.5f);
        check(failures, player.getLevel() == 5 && player.getExp() == 0.5f && player.getExpToLevel() == 17, "experience " + player.getLevel() + " " + player.getExp());
        player.setLevel(0);
        player.setExp(0f);
        final org.bukkit.Location target = player.getLocation().add(100, 0, 50);
        player.setCompassTarget(target);
        check(failures, player.getCompassTarget().getBlockX() == target.getBlockX() && player.getCompassTarget().getBlockZ() == target.getBlockZ(), "compass target");
        player.setRespawnLocation(target, true);
        check(failures, player.getPotentialRespawnLocation() != null && player.getPotentialRespawnLocation().getBlockX() == target.getBlockX() && player.getRespawnLocation() == null,
            "respawn point (no bed there)");
        player.setRespawnLocation(null);
        player.setCooldown(Material.ENDER_PEARL, 40);
        check(failures, player.hasCooldown(Material.ENDER_PEARL) && player.getCooldown(Material.ENDER_PEARL) > 30, "cooldown " + player.getCooldown(Material.ENDER_PEARL));
        player.setCooldown(Material.ENDER_PEARL, 0);
        player.setHealthScale(40);
        check(failures, player.isHealthScaled() && player.getHealthScale() == 40, "health scale");
        player.setHealthScaled(false);
        check(failures, !player.isHealthScaled(), "health not scaled");
        final org.bukkit.entity.Pig pig = player.getWorld().spawn(player.getLocation().add(3, 0, 0), org.bukkit.entity.Pig.class);
        player.hideEntity(this.plugin, pig);
        check(failures, !player.canSee(pig), "pig hidden");
        player.showEntity(this.plugin, pig);
        check(failures, player.canSee(pig), "pig shown");
        final boolean[] ran = {false};
        check(failures, pig.getScheduler().run(this.plugin, task -> ran[0] = true, null) != null && player.getScheduler().run(this.plugin, task -> { }, null) != null, "entity schedulers");
        pig.remove();
        check(failures, player.getClientOption(com.destroystokyo.paper.ClientOption.LOCALE) != null && player.getClientOption(com.destroystokyo.paper.ClientOption.MAIN_HAND) != null, "client options");
        check(failures, player.getTargetBlock(null, 10) != null && !player.getLineOfSight(null, 10).isEmpty(), "line of sight");
        player.setSleepingIgnored(true);
        check(failures, player.isSleepingIgnored(), "sleeping ignored");
        player.setSleepingIgnored(false);
        player.updateInventory();
        player.setPlayerListHeaderFooter("Head", "Foot");
        check(failures, "Head".equals(player.getPlayerListHeader()) && "Foot".equals(player.getPlayerListFooter()), "tab list");
        player.sendExperienceChange(0.25f, 3);
        player.sendTitle(new com.destroystokyo.paper.Title("Hello", "World"));
        final String part = player.getName().substring(1, 3);
        check(failures, Bukkit.matchPlayer(part).contains(player) && player.getUniqueId().equals(Bukkit.getPlayerUniqueId(player.getName())), "matchPlayer " + part);
        check(failures, Bukkit.isOwnedByCurrentRegion(player) && Bukkit.isGlobalTickThread() && Bukkit.getSpawnLimit(org.bukkit.entity.SpawnCategory.MONSTER) == 70, "regions and limits");
        report(player, "playerstate", failures);
    }

    @SuppressWarnings("deprecation")
    private void legacy(final CommandSender sender) {
        final List<String> failures = new ArrayList<>();
        final Material[] values = Material.values();
        check(failures, java.util.Arrays.stream(values).noneMatch(Material::isLegacy) && values.length > 1000, "values() without legacy materials: " + values.length);
        final String kind = switch (Material.OAK_LOG) {
            case OAK_LOG -> "log";
            case STONE -> "stone";
            default -> "other";
        };
        check(failures, kind.equals("log"), "switch over Material: " + kind);
        check(failures, Material.getMaterial("WOOL", true) == Material.WHITE_WOOL, "getMaterial(WOOL, legacy): " + Material.getMaterial("WOOL", true));
        check(failures, Material.LEGACY_WOOL.isBlock() && !Material.LEGACY_DIAMOND.isBlock() && Material.LEGACY_DIAMOND.isItem(), "legacy isBlock / isItem");
        final org.bukkit.UnsafeValues unsafe = Bukkit.getUnsafe();
        check(failures, unsafe.fromLegacy(new org.bukkit.material.MaterialData(Material.LEGACY_WOOL, (byte) 14)) == Material.RED_WOOL, "red wool");
        check(failures, unsafe.fromLegacy(new org.bukkit.material.MaterialData(Material.LEGACY_INK_SACK, (byte) 1), true) == Material.RED_DYE, "rose red is red dye");
        check(failures, unsafe.fromLegacy(Material.LEGACY_LOG, (byte) 5).getAsString().equals("minecraft:spruce_log[axis=x]"), "spruce log on its side: " + unsafe.fromLegacy(Material.LEGACY_LOG, (byte) 5).getAsString());
        check(failures, unsafe.toLegacy(Material.RED_WOOL) == Material.LEGACY_WOOL && unsafe.toLegacy(Material.DIAMOND) == Material.LEGACY_DIAMOND, "toLegacy: " + unsafe.toLegacy(Material.RED_WOOL));
        check(failures, unsafe.fromLegacy(new org.bukkit.material.MaterialData(Material.LEGACY_MONSTER_EGG, (byte) 50), true) == Material.CREEPER_SPAWN_EGG, "creeper egg");
        report(sender, "legacy", failures);
    }

    private void villagers(final Player player) {
        final List<String> failures = new ArrayList<>();
        final org.bukkit.entity.Villager villager = player.getWorld().spawn(player.getLocation().add(2, 0, 0), org.bukkit.entity.Villager.class);
        check(failures, villager.getProfession() == org.bukkit.entity.Villager.Profession.NONE, "unemployed at first: " + villager.getProfession().getKey());
        check(failures, villager.getVillagerType() == org.bukkit.entity.Villager.Type.PLAINS, "plains type: " + villager.getVillagerType().getKey());
        villager.setProfession(org.bukkit.entity.Villager.Profession.LIBRARIAN);
        villager.setVillagerType(org.bukkit.entity.Villager.Type.SNOW);
        villager.setVillagerLevel(2);
        villager.setVillagerExperience(15);
        check(failures, villager.getProfession() == org.bukkit.entity.Villager.Profession.LIBRARIAN && villager.getVillagerType() == org.bukkit.entity.Villager.Type.SNOW
            && villager.getVillagerLevel() == 2 && villager.getVillagerExperience() == 15, "villager data");
        try {
            villager.setVillagerLevel(6);
            failures.add("level 6 accepted");
        } catch (IllegalArgumentException expected) {
            // As in Paper.
        }
        // Plugins set the trades.
        villager.setRecipes(List.of(recipe(Material.BREAD, 3, Material.EMERALD, 2, 5)));
        final org.bukkit.inventory.MerchantRecipe first = villager.getRecipe(0);
        check(failures, villager.getRecipeCount() == 1 && first.getResult().getType() == Material.BREAD && first.getResult().getAmount() == 3
            && first.getIngredients().get(0).getAmount() == 2 && first.getMaxUses() == 5 && first.getVillagerExperience() == 2, "recipes");
        // New trades come from the profession's trade data, through VillagerAcquireTradeEvent
        // (the listener below raises their maximum uses).
        check(failures, villager.addTrades(1) && villager.getRecipeCount() == 2 && villager.getRecipe(1).getMaxUses() == 99, "addTrades: " + villager.getRecipes().size());
        check(failures, villager.increaseLevel(1) && villager.getVillagerLevel() == 3 && villager.getRecipeCount() > 2, "increaseLevel: " + villager.getVillagerLevel());
        final com.destroystokyo.paper.entity.villager.Reputation reputation = new com.destroystokyo.paper.entity.villager.Reputation();
        reputation.setReputation(com.destroystokyo.paper.entity.villager.ReputationType.MAJOR_POSITIVE, 10);
        villager.setReputation(player.getUniqueId(), reputation);
        check(failures, villager.getReputation(player.getUniqueId()).getReputation(com.destroystokyo.paper.entity.villager.ReputationType.MAJOR_POSITIVE) == 10, "reputation");
        villager.shakeHead();
        // The trading window (Alice trades in tests/java_plugins.rs).
        villager.setRecipes(List.of(recipe(Material.BREAD, 3, Material.EMERALD, 2, 5)));
        final org.bukkit.inventory.InventoryView view = player.openMerchant(villager, false);
        check(failures, view instanceof org.bukkit.inventory.view.MerchantView merchantView && villager.equals(merchantView.getMerchant()), "openMerchant view");
        check(failures, view != null && view.getTopInventory() instanceof org.bukkit.inventory.MerchantInventory, "merchant inventory");
        check(failures, villager.isTrading() && player.equals(villager.getTrader()), "trading with Alice");
        report(player, "villagers", failures);
    }

    private void merchant(final Player player) {
        final List<String> failures = new ArrayList<>();
        this.shop = Bukkit.createMerchant(Component.text("Shop"));
        this.shop.setRecipes(List.of(recipe(Material.DIAMOND, 1, Material.GOLD_INGOT, 4, 10)));
        check(failures, this.shop.getRecipeCount() == 1 && !this.shop.isTrading(), "a merchant with one trade");
        final org.bukkit.inventory.InventoryView view = player.openMerchant(this.shop, true);
        check(failures, view != null && this.shop.isTrading() && view.getType() == org.bukkit.event.inventory.InventoryType.MERCHANT, "openMerchant");
        report(player, "merchant", failures);
    }

    /** A villager next to a lectern takes the job (VillagerCareerChangeEvent). */
    // --- riding and pets ------------------------------------------------------------------------

    private void riding(final Player player) {
        final Location at = player.getLocation().add(4, 0, 0);
        final org.bukkit.entity.Pig pig = player.getWorld().spawn(at, org.bukkit.entity.Pig.class);
        final org.bukkit.entity.Zombie zombie = player.getWorld().spawn(at, org.bukkit.entity.Zombie.class);
        final boolean added = pig.addPassenger(zombie);
        final org.bukkit.entity.Entity vehicle = zombie.getVehicle();
        player.sendMessage("riding: added " + added + " vehicle " + (vehicle == null ? "none" : vehicle.getType().getKey().getKey()) + " passengers " + pig.getPassengers().size());
        final boolean ejected = pig.eject();
        player.sendMessage("riding: ejected " + ejected + " passengers " + pig.getPassengers().size() + " inside " + zombie.isInsideVehicle());
        zombie.remove();
        pig.remove();
        final org.bukkit.entity.Wolf wolf = player.getWorld().spawn(at, org.bukkit.entity.Wolf.class);
        wolf.setOwner(player);
        wolf.setSitting(true);
        player.sendMessage("pets: tamed " + wolf.isTamed() + " owner " + (wolf.getOwner() == null ? "none" : wolf.getOwner().getName()) + " sitting " + wolf.isSitting());
        wolf.remove();
        final org.bukkit.entity.Horse horse = player.getWorld().spawn(at, org.bukkit.entity.Horse.class);
        horse.setDomestication(42);
        horse.setJumpStrength(0.9);
        player.sendMessage("horse: domestication " + horse.getDomestication() + "/" + horse.getMaxDomestication() + " jump " + Math.round(horse.getJumpStrength() * 10));
        horse.remove();
    }

    private void career(final Player player) {
        final Location at = player.getLocation().getBlock().getLocation().add(3, 0, 0);
        at.getBlock().setType(Material.LECTERN);
        player.getWorld().spawn(at.clone().add(-1.5, 0, 0.5), org.bukkit.entity.Villager.class).setCustomName("Hired");
        player.sendMessage("career: placed");
    }

    @EventHandler
    public void onAcquireTrade(final org.bukkit.event.entity.VillagerAcquireTradeEvent event) {
        event.getRecipe().setMaxUses(99);
    }

    @EventHandler
    public void onPurchase(final io.papermc.paper.event.player.PlayerPurchaseEvent event) {
        final ItemStack result = event.getTrade().getResult();
        final String kind = event instanceof io.papermc.paper.event.player.PlayerTradeEvent trade ? "trade with " + trade.getVillager().getType().getKey().getKey() : "purchase";
        event.getPlayer().sendMessage(kind + ": " + result.getType().getKey().getKey() + " x" + result.getAmount() + " (exp " + event.isRewardingExp() + ")");
    }

    @EventHandler
    public void onTradeSelect(final org.bukkit.event.inventory.TradeSelectEvent event) {
        event.getWhoClicked().sendMessage("selected trade " + event.getIndex() + " of " + event.getMerchant().getRecipeCount());
    }

    @EventHandler
    public void onCareer(final org.bukkit.event.entity.VillagerCareerChangeEvent event) {
        Bukkit.broadcast(Component.text("career " + event.getProfession().getKey().getKey() + " " + event.getReason() + " for " + event.getEntity().getName()));
    }

    // --- resource packs ---------------------------------------------------------------------------

    private final List<String> packStatuses = new ArrayList<>();

    @EventHandler
    public void onPackStatus(final org.bukkit.event.player.PlayerResourcePackStatusEvent event) {
        this.packStatuses.add(event.getStatus().name());
    }

    // --- attributes ---------------------------------------------------------------------------

    private void attributes(final Player player) {
        final List<String> failures = new ArrayList<>();
        final org.bukkit.attribute.AttributeInstance health = player.getAttribute(org.bukkit.attribute.Attribute.MAX_HEALTH);
        check(failures, health != null && health.getBaseValue() == 20.0 && health.getValue() == 20.0 && health.getDefaultValue() == 20.0, "max health");
        final NamespacedKey bonus = new NamespacedKey(this.plugin, "bonus");
        health.addModifier(new org.bukkit.attribute.AttributeModifier(bonus, 10.0, org.bukkit.attribute.AttributeModifier.Operation.ADD_NUMBER));
        check(failures, health.getValue() == 30.0 && health.getModifier(bonus) != null && player.getMaxHealth() == 30.0, "modifier " + health.getValue());
        player.setHealth(30.0);
        check(failures, player.getHealth() == 30.0, "more health " + player.getHealth());
        health.removeModifier(bonus);
        check(failures, health.getValue() == 20.0 && player.getHealth() <= 20.0, "removed " + player.getHealth());
        final org.bukkit.attribute.AttributeInstance speed = player.getAttribute(org.bukkit.attribute.Attribute.MOVEMENT_SPEED);
        check(failures, speed != null && Math.abs(speed.getValue() - 0.1) < 1e-6, "speed " + (speed == null ? null : speed.getValue()));

        // Equipment counts: an iron chestplate gives 6 armour.
        final org.bukkit.attribute.AttributeInstance armor = player.getAttribute(org.bukkit.attribute.Attribute.ARMOR);
        final double before = armor.getValue();
        player.getInventory().setChestplate(new ItemStack(Material.IRON_CHESTPLATE));
        check(failures, armor.getValue() == before + 6.0, "armor " + before + " -> " + armor.getValue());
        player.getInventory().setChestplate(null);

        final Zombie zombie = player.getWorld().spawn(player.getLocation().add(3, 0, 0), Zombie.class);
        final org.bukkit.attribute.AttributeInstance zombieHealth = zombie.getAttribute(org.bukkit.attribute.Attribute.MAX_HEALTH);
        zombieHealth.setBaseValue(40.0);
        zombie.setHealth(40.0);
        check(failures, zombie.getMaxHealth() == 40.0 && zombie.getHealth() == 40.0, "zombie health " + zombie.getMaxHealth());
        check(failures, zombie.getAttribute(org.bukkit.attribute.Attribute.FOLLOW_RANGE).getValue() == 35.0, "follow range");
        check(failures, zombie.getAttribute(org.bukkit.attribute.Attribute.FLYING_SPEED) == null, "zombies do not fly");
        zombie.registerAttribute(org.bukkit.attribute.Attribute.FLYING_SPEED);
        check(failures, zombie.getAttribute(org.bukkit.attribute.Attribute.FLYING_SPEED) != null, "registered");
        zombie.getAttribute(org.bukkit.attribute.Attribute.SCALE).setBaseValue(2.0);
        check(failures, zombie.getAttribute(org.bukkit.attribute.Attribute.SCALE).getValue() == 2.0, "scale");
        zombie.remove();
        report(player, "attributes", failures);
    }

    // --- special item metas ---------------------------------------------------------------------

    /** Special metas survive a trip through the server: each item goes into the inventory and is read back. */
    private void metas(final Player player) {
        final List<String> failures = new ArrayList<>();
        final PlayerInventory inventory = player.getInventory();
        final java.util.function.BiFunction<Integer, ItemStack, ItemMeta> roundTrip = (slot, stack) -> {
            inventory.setItem(slot, stack);
            final ItemStack back = inventory.getItem(slot);
            return back == null ? null : back.getItemMeta();
        };

        final ItemStack head = ItemStack.of(Material.PLAYER_HEAD);
        head.editMeta(org.bukkit.inventory.meta.SkullMeta.class, m -> m.setOwningPlayer(player));
        final ItemMeta headMeta = roundTrip.apply(9, head);
        check(failures, headMeta instanceof org.bukkit.inventory.meta.SkullMeta m && m.hasOwner() && player.getName().equals(m.getOwner())
            && m.getOwningPlayer() != null && player.getUniqueId().equals(m.getOwningPlayer().getUniqueId()), "skull " + headMeta);

        final ItemStack potion = ItemStack.of(Material.POTION);
        potion.editMeta(org.bukkit.inventory.meta.PotionMeta.class, m -> {
            m.setBasePotionType(org.bukkit.potion.PotionType.STRONG_HEALING);
            m.addCustomEffect(new org.bukkit.potion.PotionEffect(org.bukkit.potion.PotionEffectType.SPEED, 200, 1), true);
            m.setColor(org.bukkit.Color.RED);
        });
        final ItemMeta potionMeta = roundTrip.apply(10, potion);
        check(failures, potionMeta instanceof org.bukkit.inventory.meta.PotionMeta m && m.getBasePotionType() == org.bukkit.potion.PotionType.STRONG_HEALING
            && m.hasCustomEffect(org.bukkit.potion.PotionEffectType.SPEED) && org.bukkit.Color.RED.equals(m.getColor())
            && m.getAllEffects().size() == 2 && org.bukkit.potion.PotionType.STRONG_HEALING.isInstant(), "potion " + potionMeta);

        final ItemStack rocket = ItemStack.of(Material.FIREWORK_ROCKET);
        final org.bukkit.FireworkEffect star = org.bukkit.FireworkEffect.builder().with(org.bukkit.FireworkEffect.Type.STAR)
            .withColor(org.bukkit.Color.RED).withFade(org.bukkit.Color.BLUE).trail(true).build();
        rocket.editMeta(org.bukkit.inventory.meta.FireworkMeta.class, m -> {
            m.setPower(2);
            m.addEffect(star);
        });
        final ItemMeta rocketMeta = roundTrip.apply(11, rocket);
        check(failures, rocketMeta instanceof org.bukkit.inventory.meta.FireworkMeta m && m.getPower() == 2 && m.getEffects().equals(List.of(star)), "firework " + rocketMeta);

        final ItemStack banner = ItemStack.of(Material.WHITE_BANNER);
        final org.bukkit.block.banner.Pattern stripe = new org.bukkit.block.banner.Pattern(org.bukkit.DyeColor.RED, org.bukkit.block.banner.PatternType.STRIPE_TOP);
        banner.editMeta(org.bukkit.inventory.meta.BannerMeta.class, m -> m.addPattern(stripe));
        final ItemMeta bannerMeta = roundTrip.apply(12, banner);
        check(failures, bannerMeta instanceof org.bukkit.inventory.meta.BannerMeta m && m.getPatterns().equals(List.of(stripe)), "banner " + bannerMeta);

        final ItemStack book = ItemStack.of(Material.WRITTEN_BOOK);
        book.editMeta(org.bukkit.inventory.meta.BookMeta.class, m -> {
            m.setTitle("Guide");
            m.setAuthor(player.getName());
            m.addPages(Component.text("Page one", NamedTextColor.GOLD), Component.text("Page two"));
        });
        final ItemMeta bookMeta = roundTrip.apply(13, book);
        check(failures, bookMeta instanceof org.bukkit.inventory.meta.BookMeta m && "Guide".equals(m.getTitle()) && player.getName().equals(m.getAuthor())
            && m.getPageCount() == 2 && Component.text("Page one", NamedTextColor.GOLD).equals(m.page(1)), "book " + bookMeta);
        check(failures, inventory.getItem(13).isSimilar(book), "book similar");

        final ItemStack draft = ItemStack.of(Material.WRITABLE_BOOK);
        draft.editMeta(org.bukkit.inventory.meta.WritableBookMeta.class, m -> m.addPage("draft"));
        final ItemMeta draftMeta = roundTrip.apply(14, draft);
        check(failures, draftMeta instanceof org.bukkit.inventory.meta.WritableBookMeta m && m.getPages().equals(List.of("draft")), "writable " + draftMeta);

        final ItemStack stairs = ItemStack.of(Material.OAK_STAIRS);
        stairs.editMeta(org.bukkit.inventory.meta.BlockDataMeta.class, m -> m.setBlockData(Bukkit.createBlockData("minecraft:oak_stairs[half=top]")));
        final ItemMeta stairsMeta = roundTrip.apply(15, stairs);
        check(failures, stairsMeta instanceof org.bukkit.inventory.meta.BlockDataMeta m && m.hasBlockData()
            && ((org.bukkit.block.data.type.Stairs) m.getBlockData(Material.OAK_STAIRS)).getHalf() == org.bukkit.block.data.Bisected.Half.TOP, "block data " + stairsMeta);

        final ItemStack stew = ItemStack.of(Material.SUSPICIOUS_STEW);
        stew.editMeta(org.bukkit.inventory.meta.SuspiciousStewMeta.class, m -> m.addCustomEffect(new org.bukkit.potion.PotionEffect(org.bukkit.potion.PotionEffectType.NIGHT_VISION, 100, 0), true));
        final ItemMeta stewMeta = roundTrip.apply(16, stew);
        check(failures, stewMeta instanceof org.bukkit.inventory.meta.SuspiciousStewMeta m && m.getCustomEffects().size() == 1
            && m.getCustomEffects().get(0).getDuration() == 100, "stew " + stewMeta);

        // A lodestone compass, a bucket of a tropical fish and one of an axolotl.
        final ItemStack compass = new ItemStack(Material.COMPASS);
        final org.bukkit.inventory.meta.CompassMeta compassMeta = (org.bukkit.inventory.meta.CompassMeta) compass.getItemMeta();
        compassMeta.setLodestone(new org.bukkit.Location(player.getWorld(), 10, -61, -20));
        compassMeta.setLodestoneTracked(false);
        compass.setItemMeta(compassMeta);
        final ItemMeta compassBack = roundTrip.apply(17, compass);
        check(failures, compassBack instanceof org.bukkit.inventory.meta.CompassMeta m && m.hasLodestone() && !m.isLodestoneTracked()
            && m.getLodestone().getBlockX() == 10 && m.getLodestone().getBlockY() == -61 && m.getLodestone().getBlockZ() == -20
            && player.getWorld().equals(m.getLodestone().getWorld()), "compass " + compassBack);
        final ItemStack fish = new ItemStack(Material.TROPICAL_FISH_BUCKET);
        final org.bukkit.inventory.meta.TropicalFishBucketMeta fishMeta = (org.bukkit.inventory.meta.TropicalFishBucketMeta) fish.getItemMeta();
        check(failures, !fishMeta.hasVariant(), "a plain fish bucket");
        fishMeta.setPattern(org.bukkit.entity.TropicalFish.Pattern.GLITTER);
        fishMeta.setBodyColor(org.bukkit.DyeColor.ORANGE);
        fishMeta.setPatternColor(org.bukkit.DyeColor.BLUE);
        fish.setItemMeta(fishMeta);
        final ItemMeta fishBack = roundTrip.apply(18, fish);
        check(failures, fishBack instanceof org.bukkit.inventory.meta.TropicalFishBucketMeta m && m.hasVariant() && m.getPattern() == org.bukkit.entity.TropicalFish.Pattern.GLITTER
            && m.getBodyColor() == org.bukkit.DyeColor.ORANGE && m.getPatternColor() == org.bukkit.DyeColor.BLUE, "fish bucket " + fishBack);
        final ItemStack axolotl = new ItemStack(Material.AXOLOTL_BUCKET);
        final org.bukkit.inventory.meta.AxolotlBucketMeta axolotlMeta = (org.bukkit.inventory.meta.AxolotlBucketMeta) axolotl.getItemMeta();
        axolotlMeta.setVariant(org.bukkit.entity.Axolotl.Variant.CYAN);
        axolotl.setItemMeta(axolotlMeta);
        final ItemMeta axolotlBack = roundTrip.apply(19, axolotl);
        check(failures, axolotlBack instanceof org.bukkit.inventory.meta.AxolotlBucketMeta m && m.hasVariant() && m.getVariant() == org.bukkit.entity.Axolotl.Variant.CYAN, "axolotl bucket " + axolotlBack);
        report(player, "metas", failures);
    }

    // --- block data ---------------------------------------------------------------------------

    private void blockData(final Player player) {
        final List<String> failures = new ArrayList<>();
        final World world = player.getWorld();
        final Location base = player.getLocation().getBlock().getLocation().add(3, 0, -3);

        final org.bukkit.block.data.BlockData parsed = Bukkit.createBlockData("minecraft:oak_stairs[facing=east,half=top]");
        check(failures, parsed instanceof org.bukkit.block.data.type.Stairs s && s.getFacing() == org.bukkit.block.BlockFace.EAST
            && s.getHalf() == org.bukkit.block.data.Bisected.Half.TOP && s.getShape() == org.bukkit.block.data.type.Stairs.Shape.STRAIGHT, "parse " + parsed);
        check(failures, Material.WHEAT.createBlockData() instanceof org.bukkit.block.data.Ageable a && a.getMaximumAge() == 7, "wheat");
        check(failures, Bukkit.createBlockData(Material.OAK_LOG, "[axis=x]") instanceof org.bukkit.block.data.Orientable o && o.getAxis() == org.bukkit.Axis.X, "log");
        check(failures, org.bukkit.block.BlockType.CHEST.createBlockData() instanceof org.bukkit.block.data.type.Chest, "block type");

        // Block#setBlockData and getBlockData, with and without physics.
        final Block block = world.getBlockAt(base);
        block.setBlockData(parsed);
        check(failures, block.getType() == Material.OAK_STAIRS, "type " + block.getType());
        check(failures, block.getBlockData().matches(parsed), "read back " + block.getBlockData().getAsString());
        final org.bukkit.block.data.type.Stairs stairs = (org.bukkit.block.data.type.Stairs) block.getBlockData();
        stairs.setFacing(org.bukkit.block.BlockFace.NORTH);
        block.setBlockData(stairs, false);
        check(failures, ((org.bukkit.block.data.Directional) block.getBlockData()).getFacing() == org.bukkit.block.BlockFace.NORTH, "facing");
        final Location fence = base.clone().add(0, 0, -1);
        world.setBlockData(fence, Material.OAK_FENCE.createBlockData());
        check(failures, world.getBlockData(fence) instanceof org.bukkit.block.data.type.Fence, "world " + world.getBlockData(fence).getAsString());

        // Block states are snapshots that update() writes back.
        final BlockState state = block.getState();
        state.setType(Material.GOLD_BLOCK);
        check(failures, block.getType() == Material.OAK_STAIRS, "snapshot applied early");
        check(failures, !state.update(), "update without force");
        check(failures, state.update(true) && block.getType() == Material.GOLD_BLOCK, "forced update " + block.getType());

        try {
            block.getBlockData().getSoundGroup();
            failures.add("getSoundGroup returned");
        } catch (UnsupportedOperationException e) {
            check(failures, e.getMessage().contains("BlockData#getSoundGroup"), e.getMessage());
        }
        // A block only this player sees.
        player.sendBlockChange(base.clone().add(0, 1, 0), Material.DIAMOND_BLOCK.createBlockData());
        report(player, "blockdata", failures);
    }

    /** BlockPlaceEvent shows the placed block; stairs are turned upside down. */
    @EventHandler
    public void onPlace(final org.bukkit.event.block.BlockPlaceEvent event) {
        if (event.getBlockPlaced().getType() != Material.OAK_STAIRS) {
            return;
        }
        event.getPlayer().sendMessage("place: " + event.getBlockPlaced().getType() + " over " + event.getBlockReplacedState().getType()
            + " with " + event.getItemInHand().getType());
        final org.bukkit.block.data.type.Stairs stairs = (org.bukkit.block.data.type.Stairs) event.getBlockPlaced().getBlockData();
        stairs.setHalf(org.bukkit.block.data.Bisected.Half.TOP);
        event.getBlockPlaced().setBlockData(stairs);
    }

    // --- potion effects and boss bars ---------------------------------------------------------

    private void effects(final Player player) {
        final List<String> failures = new ArrayList<>();
        check(failures, player.addPotionEffect(new org.bukkit.potion.PotionEffect(org.bukkit.potion.PotionEffectType.SPEED, 600, 1)), "add");
        check(failures, player.hasPotionEffect(org.bukkit.potion.PotionEffectType.SPEED), "has");
        final org.bukkit.potion.PotionEffect speed = player.getPotionEffect(org.bukkit.potion.PotionEffectType.SPEED);
        check(failures, speed != null && speed.getAmplifier() == 1 && speed.getDuration() > 500, "get " + speed);
        check(failures, player.getActivePotionEffects().size() == 1, "active " + player.getActivePotionEffects());
        player.removePotionEffect(org.bukkit.potion.PotionEffectType.SPEED);
        check(failures, !player.hasPotionEffect(org.bukkit.potion.PotionEffectType.SPEED), "removed");
        player.addPotionEffect(new org.bukkit.potion.PotionEffect(org.bukkit.potion.PotionEffectType.GLOWING, 2000, 0));
        final Zombie zombie = player.getWorld().spawn(player.getLocation().add(3, 0, 0), Zombie.class);
        check(failures, zombie.addPotionEffect(new org.bukkit.potion.PotionEffect(org.bukkit.potion.PotionEffectType.SLOWNESS, 200, 2)), "mob add");
        check(failures, zombie.hasPotionEffect(org.bukkit.potion.PotionEffectType.SLOWNESS), "mob has");
        zombie.remove();
        report(player, "effects", failures);
    }

    private void bossbar(final Player player) {
        final List<String> failures = new ArrayList<>();
        final net.kyori.adventure.bossbar.BossBar raid = net.kyori.adventure.bossbar.BossBar.bossBar(Component.text("Raid"), 0.5f, net.kyori.adventure.bossbar.BossBar.Color.RED, net.kyori.adventure.bossbar.BossBar.Overlay.NOTCHED_10);
        player.showBossBar(raid);
        check(failures, player.activeBossBars().iterator().hasNext(), "active");
        raid.progress(0.25f);
        final NamespacedKey key = new NamespacedKey(this.plugin, "event");
        final org.bukkit.boss.KeyedBossBar event = Bukkit.createBossBar(key, "Event", org.bukkit.boss.BarColor.BLUE, org.bukkit.boss.BarStyle.SOLID);
        event.addPlayer(player);
        check(failures, event.getPlayers().contains(player), "bukkit players");
        check(failures, Bukkit.getBossBar(key) == event, "by key");
        report(player, "bossbar", failures);
    }

    // --- items and inventories ----------------------------------------------------------------

    private void items(final Player player) {
        final List<String> failures = new ArrayList<>();
        final ItemStack sword = ItemStack.of(Material.DIAMOND_SWORD);
        sword.editMeta(meta -> {
            meta.displayName(Component.text("Excalibur", NamedTextColor.GOLD));
            meta.lore(List.of(Component.text("Legendary"), Component.text("Sharp")));
            meta.addEnchant(Enchantment.SHARPNESS, 5, true);
            meta.setUnbreakable(true);
            meta.setCustomModelData(7);
            meta.addItemFlags(ItemFlag.HIDE_ENCHANTS);
            meta.getPersistentDataContainer().set(this.ownerKey, PersistentDataType.STRING, player.getName());
            ((Damageable) meta).setDamage(10);
        });
        final PlayerInventory inventory = player.getInventory();
        inventory.clear();
        inventory.setItem(0, sword);

        final ItemStack read = inventory.getItem(0);
        check(failures, read != null && read.getType() == Material.DIAMOND_SWORD, "type");
        final ItemMeta meta = read.getItemMeta();
        check(failures, Component.text("Excalibur", NamedTextColor.GOLD).equals(meta.displayName()), "name " + meta.displayName());
        check(failures, meta.lore() != null && meta.lore().size() == 2, "lore");
        check(failures, meta.getEnchantLevel(Enchantment.SHARPNESS) == 5, "enchantment");
        check(failures, meta.isUnbreakable(), "unbreakable");
        check(failures, meta.hasCustomModelData() && meta.getCustomModelData() == 7, "custom model data");
        check(failures, meta.hasItemFlag(ItemFlag.HIDE_ENCHANTS), "item flag");
        check(failures, player.getName().equals(meta.getPersistentDataContainer().get(this.ownerKey, PersistentDataType.STRING)), "persistent data");
        check(failures, meta instanceof Damageable d && d.getDamage() == 10, "damage");
        check(failures, read.isSimilar(sword), "similar");
        try {
            check(failures, Enchantment.SHARPNESS.getMaxLevel() == 5 && Enchantment.SHARPNESS.conflictsWith(Enchantment.SMITE), "enchantment data");
        } catch (UnsupportedOperationException e) {
            // Servers without vanilla game data do not know enchantment levels.
        }

        // Mirror stacks write through, as on Paper.
        read.setAmount(1);
        inventory.setHeldItemSlot(0);
        check(failures, inventory.getHeldItemSlot() == 0, "held slot");
        final ItemStack named = inventory.getItemInMainHand();
        check(failures, named.getType() == Material.DIAMOND_SWORD, "main hand");

        final Map<Integer, ItemStack> leftover = inventory.addItem(new ItemStack(Material.STONE, 70), new ItemStack(Material.DIRT, 3));
        check(failures, leftover.isEmpty(), "addItem leftover " + leftover);
        check(failures, inventory.contains(Material.STONE, 70), "contains");
        check(failures, inventory.first(Material.STONE) == 1, "first " + inventory.first(Material.STONE));
        final Map<Integer, ItemStack> missing = inventory.removeItem(new ItemStack(Material.STONE, 5));
        check(failures, missing.isEmpty() && inventory.containsAtLeast(new ItemStack(Material.STONE), 65) && !inventory.containsAtLeast(new ItemStack(Material.STONE), 66), "removeItem");
        inventory.setHelmet(new ItemStack(Material.IRON_HELMET));
        check(failures, inventory.getHelmet() != null && inventory.getHelmet().getType() == Material.IRON_HELMET, "helmet");
        check(failures, inventory.getArmorContents()[3].getType() == Material.IRON_HELMET, "armor contents");

        player.getEnderChest().setItem(4, new ItemStack(Material.EMERALD, 2));
        check(failures, player.getEnderChest().getItem(4) != null && player.getEnderChest().getItem(4).getAmount() == 2, "ender chest");

        final ItemStack copy = ItemStack.deserializeBytes(sword.serializeAsBytes());
        check(failures, copy.isSimilar(sword), "serializeAsBytes");
        final ItemStack fromMap = ItemStack.deserialize(sword.serialize());
        check(failures, fromMap.isSimilar(sword), "serialize");
        report(player, "items", failures);
    }

    private void gui(final Player player) {
        final TestMenu holder = new TestMenu();
        final Inventory menu = Bukkit.createInventory(holder, 27, Component.text("Test Menu"));
        holder.inventory = menu;
        final ItemStack button = new ItemStack(Material.EMERALD);
        button.editMeta(meta -> meta.displayName(Component.text("Click me")));
        menu.setItem(13, button);
        player.openInventory(menu);
        player.sendMessage("gui: open " + (player.getOpenInventory().getTopInventory().getHolder() == holder));
    }

    private void chest(final CommandSender sender, final Block block) {
        final List<String> failures = new ArrayList<>();
        final BlockState state = block.getState();
        check(failures, state instanceof Chest, "state " + state);
        if (state instanceof Chest chest) {
            chest.getInventory().setItem(0, new ItemStack(Material.DIAMOND, 3));
            check(failures, chest.getInventory().contains(Material.DIAMOND, 3), "contents");
            check(failures, chest.getInventory().getSize() == 27, "size");
        }
        report(sender, "chest", failures);
    }

    @EventHandler
    public void onOpen(final InventoryOpenEvent event) {
        if (event.getInventory().getHolder() instanceof TestMenu) {
            event.getPlayer().sendMessage("opened Test Menu");
        }
    }

    @EventHandler
    public void onClick(final InventoryClickEvent event) {
        if (!(event.getInventory().getHolder() instanceof TestMenu)) {
            return;
        }
        event.setCancelled(true);
        final ItemStack current = event.getCurrentItem();
        if (event.getRawSlot() == 13 && current != null && current.getType() == Material.EMERALD) {
            event.getWhoClicked().sendMessage("clicked " + current.getType().getKey().getKey() + " with " + event.getClick());
            event.getWhoClicked().closeInventory();
        }
    }

    @EventHandler
    public void onClose(final InventoryCloseEvent event) {
        if (event.getInventory().getHolder() instanceof TestMenu) {
            event.getPlayer().sendMessage("menu closed (" + event.getReason() + ")");
        }
    }

    @EventHandler
    public void onCraft(final org.bukkit.event.inventory.CraftItemEvent event) {
        final ItemStack result = event.getRecipe().getResult();
        event.getWhoClicked().sendMessage("crafted " + result.getType().getKey().getKey() + " x" + result.getAmount() + " in " + event.getInventory().getType()
            + " (" + event.getClick() + ", " + event.getAction() + ")");
    }

    @EventHandler
    public void onDrag(final org.bukkit.event.inventory.InventoryDragEvent event) {
        event.getWhoClicked().sendMessage("dragged over " + event.getRawSlots().size() + " slots (" + event.getType() + ")");
    }

    @EventHandler
    public void onDrop(final PlayerDropItemEvent event) {
        if (event.getItemDrop().getItemStack().getType() == Material.DIRT) {
            event.setCancelled(true);
            event.getPlayer().sendMessage("dirt stays with you");
        }
    }

    @EventHandler
    public void onPickup(final EntityPickupItemEvent event) {
        if (event.getEntity() instanceof Player player) {
            player.sendMessage("picked up " + event.getItem().getItemStack().getType().getKey().getKey());
        }
    }

    // --- entities -----------------------------------------------------------------------------

    private void entities(final Player player) {
        final List<String> failures = new ArrayList<>();
        final World world = player.getWorld();
        final Location at = player.getLocation().add(3, 0, 0);
        final Zombie zombie = world.spawn(at, Zombie.class, z -> z.customName(Component.text("Bob the Zombie")));
        check(failures, zombie.isValid(), "valid");
        check(failures, Component.text("Bob the Zombie").equals(zombie.customName()), "custom name");
        check(failures, zombie.getMaxHealth() == 20, "max health " + zombie.getMaxHealth());
        zombie.setHealth(10);
        check(failures, zombie.getHealth() == 10, "health " + zombie.getHealth());
        zombie.getEquipment().setHelmet(new ItemStack(Material.GOLDEN_HELMET));
        check(failures, zombie.getEquipment().getHelmet().getType() == Material.GOLDEN_HELMET, "equipment");
        zombie.setTarget(player);
        check(failures, player.equals(zombie.getTarget()), "target");
        zombie.damage(4, player);
        check(failures, zombie.getHealth() < 10, "damage " + zombie.getHealth());
        final Item item = world.dropItem(at.clone().add(0, 1, 0), new ItemStack(Material.APPLE, 2));
        check(failures, item.getItemStack().getAmount() == 2, "item stack");
        item.setPickupDelay(1000);
        check(failures, item.getPickupDelay() == 1000, "pickup delay");
        final Collection<Entity> nearby = world.getNearbyEntities(at, 2, 2, 2);
        check(failures, nearby.contains(zombie) && nearby.contains(item), "nearby " + nearby);
        check(failures, world.getEntitiesByClass(Zombie.class).contains(zombie), "by class");
        check(failures, world.getLivingEntities().contains(zombie) && world.getLivingEntities().contains(player), "living");
        final Location moved = at.clone().add(0, 0, 2);
        zombie.teleport(moved);
        check(failures, zombie.getLocation().distanceSquared(moved) < 0.01, "teleport");
        item.remove();
        check(failures, !item.isValid(), "remove");
        report(player, "entities", failures);
    }

    @EventHandler
    public void onCreatureSpawn(final CreatureSpawnEvent event) {
        if (event.getSpawnReason() == CreatureSpawnEvent.SpawnReason.CUSTOM) {
            Bukkit.broadcast(Component.text("spawned " + event.getEntityType().getKey().getKey()));
        }
    }

    @EventHandler
    public void onDamage(final EntityDamageByEntityEvent event) {
        if (event.getDamager() instanceof Player player && event.getEntity() instanceof Zombie) {
            player.sendMessage("hit " + event.getEntity().getName() + " for " + (int) event.getDamage() + " (" + event.getCause() + ")");
        }
    }

    @EventHandler
    public void onEntityDeath(final EntityDeathEvent event) {
        if (event.getEntity() instanceof Zombie zombie && Component.text("Bob the Zombie").equals(zombie.customName())) {
            event.getDrops().clear();
            event.getDrops().add(new ItemStack(Material.DIAMOND, 1));
            event.setDroppedExp(0);
        }
    }

    @EventHandler
    public void onPlayerDeath(final PlayerDeathEvent event) {
        event.deathMessage(Component.text(event.getPlayer().getName() + " was defeated (api test)"));
        event.setKeepInventory(true);
        event.getDrops().clear();
    }

    @EventHandler
    public void onRespawn(final PlayerRespawnEvent event) {
        event.setRespawnLocation(event.getRespawnLocation().clone().add(0, 5, 0));
    }

    // --- offline players and scoreboards ------------------------------------------------------

    private void offline(final CommandSender sender, final String name) {
        final OfflinePlayer player = Bukkit.getOfflinePlayer(name);
        sender.sendMessage("offline " + player.getName() + " played=" + player.hasPlayedBefore() + " online=" + player.isOnline()
            + " first=" + (player.getFirstPlayed() > 0) + " last=" + (player.getLastPlayed() > 0) + " known=" + Bukkit.getOfflinePlayers().length);
    }

    private void scoreboard(final Player player) {
        final List<String> failures = new ArrayList<>();
        final Scoreboard main = Bukkit.getScoreboardManager().getMainScoreboard();
        Objective kills = main.getObjective("kills");
        if (kills == null) {
            kills = main.registerNewObjective("kills", Criteria.DUMMY, Component.text("Kills"));
        }
        kills.setDisplaySlot(DisplaySlot.SIDEBAR);
        kills.getScore(player.getName()).setScore(3);
        check(failures, kills.getScore(player.getName()).getScore() == 3, "score");
        check(failures, main.getObjective(DisplaySlot.SIDEBAR) != null, "display slot");
        Team red = main.getTeam("red");
        if (red == null) {
            red = main.registerNewTeam("red");
        }
        red.prefix(Component.text("[R] "));
        red.color(NamedTextColor.RED);
        red.addEntry(player.getName());
        check(failures, red.hasEntry(player.getName()) && main.getEntryTeam(player.getName()) != null, "team");
        check(failures, NamedTextColor.RED.equals(red.color()), "team colour");
        // A trigger objective's score is enabled for /trigger by setTriggerable.
        Objective vote = main.getObjective("vote");
        if (vote == null) {
            vote = main.registerNewObjective("vote", Criteria.TRIGGER, Component.text("Vote"));
        }
        final org.bukkit.scoreboard.Score ballot = vote.getScore(player.getName());
        check(failures, !ballot.isTriggerable() && !ballot.isScoreSet(), "not triggerable before");
        ballot.setTriggerable(true);
        check(failures, ballot.isTriggerable() && ballot.isScoreSet() && ballot.getScore() == 0, "triggerable");
        try {
            kills.getScore(player.getName()).setTriggerable(true);
            failures.add("a dummy objective became triggerable");
        } catch (IllegalArgumentException expected) {
            // Only trigger objectives.
        }

        final Scoreboard personal = Bukkit.getScoreboardManager().getNewScoreboard();
        final Objective mine = personal.registerNewObjective("personal", Criteria.DUMMY, Component.text("Yours"));
        mine.setDisplaySlot(DisplaySlot.SIDEBAR);
        mine.getScore("coins").setScore(42);
        player.setScoreboard(personal);
        check(failures, player.getScoreboard() == personal, "player scoreboard");
        player.setScoreboard(main);
        report(player, "scoreboard", failures);
    }

    @EventHandler
    public void onServerLoad(final ServerLoadEvent event) {
        this.plugin.getLogger().info("Server loaded: " + event.getType() + " with " + Bukkit.getOnlinePlayers().size() + " player(s) online");
        for (Player player : Bukkit.getOnlinePlayers()) {
            player.sendMessage("server loaded: " + event.getType());
        }
    }

    @SuppressWarnings("unused")
    private static boolean living(final Entity entity) {
        return entity instanceof LivingEntity;
    }
}
