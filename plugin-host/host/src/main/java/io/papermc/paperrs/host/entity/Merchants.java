package io.papermc.paperrs.host.entity;

import com.destroystokyo.paper.entity.villager.Reputation;
import com.destroystokyo.paper.entity.villager.ReputationType;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.item.HostItemStack;
import java.util.ArrayList;
import java.util.EnumMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.MerchantRecipe;

/**
 * Trades as the server and plugins exchange them: a {@link MerchantRecipe} is a map with the
 * result, the one or two ingredients (an ingredient's components are what the payment must
 * have), uses, maximum uses, the experience reward, the villager's experience, the price
 * multiplier, demand, special price and whether discounts are ignored (Paper's
 * {@code CraftMerchantRecipe}).
 */
public final class Merchants {
    private Merchants() {
    }

    public static MerchantRecipe recipe(final Map<String, Object> v) {
        final MerchantRecipe recipe = new MerchantRecipe(HostItemStack.fromValue(v.get("result")), Values.i32(v, "uses", 0), Values.i32(v, "max_uses", 4),
            Values.bool(v, "reward_exp", true), Values.i32(v, "villager_xp", 1), (float) Values.f64(v, "price_multiplier", 0), Values.i32(v, "demand", 0),
            Values.i32(v, "special_price", 0), Values.bool(v, "ignore_discounts", false));
        for (final Object ingredient : Values.list(v.get("ingredients"))) {
            recipe.addIngredient(HostItemStack.fromValue(ingredient));
        }
        return recipe;
    }

    public static Map<String, Object> value(final MerchantRecipe recipe) {
        if (recipe == null) {
            throw new IllegalArgumentException("Recipe cannot be null");
        }
        final List<ItemStack> ingredients = recipe.getIngredients();
        if (ingredients.isEmpty()) {
            throw new IllegalStateException("No offered ingredients");
        }
        final List<Object> costs = new ArrayList<>();
        for (final ItemStack ingredient : ingredients.subList(0, Math.min(2, ingredients.size()))) {
            costs.add(HostItemStack.toValue(ingredient));
        }
        return Values.of("result", HostItemStack.toValue(recipe.getResult()), "ingredients", costs, "uses", recipe.getUses(), "max_uses", recipe.getMaxUses(),
            "reward_exp", recipe.hasExperienceReward(), "villager_xp", recipe.getVillagerExperience(), "price_multiplier", (double) recipe.getPriceMultiplier(),
            "demand", recipe.getDemand(), "special_price", recipe.getSpecialPrice(), "ignore_discounts", recipe.shouldIgnoreDiscounts());
    }

    public static List<MerchantRecipe> recipes(final Object list) {
        final List<MerchantRecipe> out = new ArrayList<>();
        for (final Object o : Values.list(list)) {
            out.add(recipe(Values.map(o)));
        }
        return List.copyOf(out);
    }

    public static List<Object> values(final List<MerchantRecipe> recipes) {
        if (recipes == null) {
            throw new IllegalArgumentException("Recipes cannot be null");
        }
        final List<Object> out = new ArrayList<>();
        for (final MerchantRecipe recipe : recipes) {
            out.add(value(recipe));
        }
        return out;
    }

    /** The merchant a server reference names: a villager or trader, or a plugin's merchant. */
    public static org.bukkit.inventory.@org.jetbrains.annotations.Nullable Merchant merchant(final org.bukkit.craftbukkit.CraftServer server, final Object ref) {
        if (!(ref instanceof Map<?, ?> m)) {
            return null;
        }
        final Map<String, Object> map = Values.map(m);
        if (map.get("merchant") instanceof Number id) {
            return HostMerchant.of(id.longValue());
        }
        return Entities.of(server, map) instanceof org.bukkit.inventory.Merchant merchant ? merchant : null;
    }

    /** A player's reputation from the server's gossip values (`major_negative`, ...). */
    public static Reputation reputation(final Object value) {
        final Map<ReputationType, Integer> values = new EnumMap<>(ReputationType.class);
        if (value instanceof Map<?, ?> m) {
            for (final Map.Entry<String, Object> e : Values.map(m).entrySet()) {
                values.put(ReputationType.valueOf(e.getKey().toUpperCase(Locale.ROOT)), ((Number) e.getValue()).intValue());
            }
        }
        return new Reputation(values);
    }

    public static Map<String, Object> reputationValue(final Reputation reputation) {
        final Map<String, Object> out = new java.util.LinkedHashMap<>();
        for (final ReputationType type : ReputationType.values()) {
            final int v = reputation.getReputation(type);
            if (v != 0) {
                out.put(type.name().toLowerCase(Locale.ROOT), v);
            }
        }
        return out;
    }
}
