package io.papermc.paperrs.host.event;

import io.papermc.paper.event.player.PlayerPurchaseEvent;
import io.papermc.paper.event.player.PlayerTradeEvent;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.entity.Entities;
import io.papermc.paperrs.host.entity.HostPlayer;
import io.papermc.paperrs.host.entity.Merchants;
import java.util.Map;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.AbstractVillager;
import org.bukkit.entity.Entity;
import org.bukkit.entity.Villager;
import org.bukkit.event.entity.VillagerAcquireTradeEvent;
import org.bukkit.event.entity.VillagerCareerChangeEvent;
import org.bukkit.event.entity.VillagerReplenishTradeEvent;
import org.bukkit.event.inventory.TradeSelectEvent;
import org.bukkit.inventory.Merchant;
import org.bukkit.inventory.MerchantRecipe;
import org.bukkit.inventory.view.MerchantView;
import org.jetbrains.annotations.Nullable;

/** Trading and villager events the server fires (see {@code plugins::events}). */
final class MerchantEvents {
    private final CraftServer server;

    MerchantEvents(final CraftServer server) {
        this.server = server;
    }

    private @Nullable Entity entity(final Map<String, Object> args) {
        return Entities.of(this.server, Values.of("entity", args.get("entity"), "value", args.get("value")));
    }

    private HostPlayer player(final Map<String, Object> args) {
        final HostPlayer player = this.server.player(Values.uuid(args, "uuid"));
        if (player == null) {
            throw new IllegalArgumentException("player " + args.get("uuid") + " is not online");
        }
        return player;
    }

    /** {@code PlayerTradeEvent} (villagers, wandering traders) or {@code PlayerPurchaseEvent}. */
    Object trade(final Map<String, Object> args) {
        final HostPlayer player = this.player(args);
        final Merchant merchant = Merchants.merchant(this.server, args.get("merchant"));
        final MerchantRecipe recipe = Merchants.recipe(Values.map(args.get("recipe")));
        final boolean reward = Values.bool(args, "reward_exp", true);
        final boolean uses = Values.bool(args, "increase_uses", true);
        if (merchant == null) {
            return Values.of("cancelled", false, "recipe", Merchants.value(recipe), "reward_exp", reward, "increase_uses", uses);
        }
        final PlayerPurchaseEvent event = merchant instanceof AbstractVillager villager
            ? new PlayerTradeEvent(player, villager, recipe, reward, uses)
            : new PlayerPurchaseEvent(player, merchant, recipe, reward, uses);
        event.callEvent();
        return Values.of("cancelled", event.isCancelled(), "recipe", Merchants.value(event.getTrade()), "reward_exp", event.isRewardingExp(),
            "increase_uses", event.willIncreaseTradeUses());
    }

    /** {@code TradeSelectEvent}. */
    Object select(final Map<String, Object> args) {
        final HostPlayer player = this.player(args);
        if (!(player.getOpenInventory() instanceof MerchantView view)) {
            return Values.of("cancelled", false);
        }
        final TradeSelectEvent event = new TradeSelectEvent(view, Values.i32(args, "index", 0));
        event.callEvent();
        return Values.of("cancelled", event.isCancelled());
    }

    /** {@code VillagerAcquireTradeEvent}, one per new trade. */
    Object acquire(final Map<String, Object> args) {
        final MerchantRecipe recipe = Merchants.recipe(Values.map(args.get("recipe")));
        if (!(this.entity(args) instanceof AbstractVillager villager)) {
            return Values.of("cancelled", false, "recipe", Merchants.value(recipe));
        }
        final VillagerAcquireTradeEvent event = new VillagerAcquireTradeEvent(villager, recipe);
        event.callEvent();
        // Paper drops a recipe a plugin left without ingredients.
        final boolean keep = !event.isCancelled() && !event.getRecipe().getIngredients().isEmpty();
        return Values.of("cancelled", !keep, "recipe", keep ? Merchants.value(event.getRecipe()) : null);
    }

    /** {@code VillagerCareerChangeEvent}. */
    Object careerChange(final Map<String, Object> args) {
        final String profession = Values.str(args, "profession", "minecraft:none");
        if (!(this.entity(args) instanceof Villager villager)) {
            return Values.of("cancelled", false, "profession", profession);
        }
        final VillagerCareerChangeEvent.ChangeReason reason = VillagerCareerChangeEvent.ChangeReason.valueOf(Values.str(args, "reason", "EMPLOYED"));
        final VillagerCareerChangeEvent event = new VillagerCareerChangeEvent(villager, Registry.VILLAGER_PROFESSION.getOrThrow(NamespacedKey.fromString(profession)), reason);
        event.callEvent();
        return Values.of("cancelled", event.isCancelled(), "profession", event.getProfession().getKey().toString());
    }

    /** {@code VillagerReplenishTradeEvent}, one per trade when a villager restocks. */
    Object replenish(final Map<String, Object> args) {
        final MerchantRecipe recipe = Merchants.recipe(Values.map(args.get("recipe")));
        if (!(this.entity(args) instanceof AbstractVillager villager)) {
            return Values.of("cancelled", false, "recipe", Merchants.value(recipe));
        }
        final VillagerReplenishTradeEvent event = new VillagerReplenishTradeEvent(villager, recipe);
        event.callEvent();
        return Values.of("cancelled", event.isCancelled(), "recipe", Merchants.value(event.getRecipe()));
    }
}
