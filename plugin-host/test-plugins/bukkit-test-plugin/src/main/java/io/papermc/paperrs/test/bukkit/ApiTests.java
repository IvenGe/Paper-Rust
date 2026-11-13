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
                case "unsupported" -> {
                    try {
                        ((Player) sender).getInventory().getItemInMainHand().getItemMeta().getAttributeModifiers();
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
