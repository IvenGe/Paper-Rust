package io.papermc.paperrs.host.item;

import io.papermc.paperrs.host.bridge.Values;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.bukkit.Keyed;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.inventory.BlastingRecipe;
import org.bukkit.inventory.CampfireRecipe;
import org.bukkit.inventory.CookingRecipe;
import org.bukkit.inventory.FurnaceRecipe;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.Recipe;
import org.bukkit.inventory.RecipeChoice;
import org.bukkit.inventory.ShapedRecipe;
import org.bukkit.inventory.ShapelessRecipe;
import org.bukkit.inventory.SmokingRecipe;
import org.bukkit.inventory.TransmuteRecipe;
import org.jetbrains.annotations.Nullable;

/**
 * Recipes between the API and the server. Plugins' recipes go to the server as maps: shaped,
 * shapeless, transmute and cooking recipes, whose ingredients are item types or exact items
 * ({@code ExactChoice}: the same item with the same components). The server's own recipes
 * come back the same way and become Bukkit recipes.
 */
public final class Recipes {
    private Recipes() {
    }

    /** A recipe as the server takes it; null for kinds the server does not craft. */
    public static @Nullable Map<String, Object> toValue(final Recipe recipe) {
        if (!(recipe instanceof Keyed keyed)) {
            return null;
        }
        final Map<String, Object> out = Values.of("key", keyed.getKey().asString(), "result", HostItemStack.toValue(recipe.getResult()));
        switch (recipe) {
            case ShapedRecipe shaped -> {
                out.put("type", "shaped");
                out.put("rows", List.of(shaped.getShape()));
                final Map<String, Object> keys = new LinkedHashMap<>();
                for (final Map.Entry<Character, RecipeChoice> entry : shaped.getChoiceMap().entrySet()) {
                    if (entry.getValue() != null && entry.getValue() != RecipeChoice.empty()) {
                        final Map<String, Object> choice = choice(entry.getValue());
                        if (choice == null) {
                            return null;
                        }
                        keys.put(String.valueOf(entry.getKey()), choice);
                    }
                }
                out.put("keys", keys);
            }
            case ShapelessRecipe shapeless -> {
                out.put("type", "shapeless");
                final List<Object> ingredients = new ArrayList<>();
                for (final RecipeChoice choice : shapeless.getChoiceList()) {
                    final Map<String, Object> value = choice(choice);
                    if (value == null) {
                        return null;
                    }
                    ingredients.add(value);
                }
                out.put("ingredients", ingredients);
            }
            case TransmuteRecipe transmute -> {
                out.put("type", "transmute");
                out.put("input", choice(transmute.getInput()));
                out.put("material", choice(transmute.getMaterial()));
                if (out.get("input") == null || out.get("material") == null) {
                    return null;
                }
            }
            case CookingRecipe<?> cooking -> {
                out.put("type", switch (cooking) {
                    case BlastingRecipe b -> "blasting";
                    case SmokingRecipe s -> "smoking";
                    case CampfireRecipe c -> "campfire_cooking";
                    default -> "smelting";
                });
                out.put("input", choice(cooking.getInputChoice()));
                if (out.get("input") == null) {
                    return null;
                }
                out.put("experience", (double) cooking.getExperience());
                out.put("cooking_time", cooking.getCookingTime());
            }
            default -> {
                return null;
            }
        }
        return out;
    }

    /** An ingredient: item types, or exact items; null for a predicate (only the host could test it). */
    private static @Nullable Map<String, Object> choice(final RecipeChoice choice) {
        return switch (choice) {
            case RecipeChoice.ExactChoice exact -> Values.of("exact", exact.getChoices().stream().map(HostItemStack::toValue).toList());
            case RecipeChoice.MaterialChoice materials -> Values.of("items", materials.getChoices().stream().map(m -> m.getKey().asString()).toList());
            case RecipeChoice.ItemTypeChoice types -> Values.of("items", types.itemTypes().values().stream().map(k -> k.key().asString()).toList());
            default -> null;
        };
    }

    /** A recipe the server sent, as a Bukkit recipe; null for one the API cannot show. */
    public static @Nullable Recipe fromValue(final Map<String, Object> value) {
        final NamespacedKey key = NamespacedKey.fromString(Values.str(value, "key"));
        final ItemStack result = HostItemStack.fromValue(value.get("result"));
        if (key == null || result.isEmpty()) {
            return null;
        }
        try {
            return switch (Values.str(value, "type", "")) {
                case "shaped" -> {
                    final ShapedRecipe recipe = new ShapedRecipe(key, result);
                    recipe.shape(Values.list(value.get("rows")).stream().map(String::valueOf).toArray(String[]::new));
                    for (final Map.Entry<String, Object> entry : Values.map(value.get("keys")).entrySet()) {
                        recipe.setIngredient(entry.getKey().charAt(0), choiceFrom(Values.map(entry.getValue())));
                    }
                    yield recipe;
                }
                case "shapeless" -> {
                    final ShapelessRecipe recipe = new ShapelessRecipe(key, result);
                    for (final Object ingredient : Values.list(value.get("ingredients"))) {
                        recipe.addIngredient(choiceFrom(Values.map(ingredient)));
                    }
                    yield recipe;
                }
                case "transmute" -> new TransmuteRecipe(key, result.getType(), choiceFrom(Values.map(value.get("input"))), choiceFrom(Values.map(value.get("material"))));
                case "smelting", "blasting", "smoking", "campfire_cooking" -> {
                    final RecipeChoice input = choiceFrom(Values.map(value.get("input")));
                    final float experience = (float) Values.f64(value, "experience", 0);
                    final int time = Values.i32(value, "cooking_time", 200);
                    yield switch (Values.str(value, "type", "")) {
                        case "blasting" -> new BlastingRecipe(key, result, input, experience, time);
                        case "smoking" -> new SmokingRecipe(key, result, input, experience, time);
                        case "campfire_cooking" -> new CampfireRecipe(key, result, input, experience, time);
                        default -> new FurnaceRecipe(key, result, input, experience, time);
                    };
                }
                default -> null;
            };
        } catch (final IllegalArgumentException e) {
            // A shape or ingredient the API does not allow (e.g. an ingredient no item has).
            return null;
        }
    }

    private static RecipeChoice choiceFrom(final Map<String, Object> value) {
        final List<Object> exact = Values.list(value.get("exact"));
        if (!exact.isEmpty()) {
            return new RecipeChoice.ExactChoice(exact.stream().map(HostItemStack::fromValue).map(ItemStack.class::cast).toList());
        }
        final List<Material> materials = new ArrayList<>();
        for (final Object item : Values.list(value.get("items"))) {
            final Material material = Material.matchMaterial(String.valueOf(item));
            if (material != null && material.isItem() && !material.isAir()) {
                materials.add(material);
            }
        }
        return new RecipeChoice.MaterialChoice(materials);
    }
}
