package io.papermc.paperrs.host.entity;

import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.bridge.Values;
import java.lang.ref.Cleaner;
import java.lang.ref.WeakReference;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import net.kyori.adventure.text.Component;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.HumanEntity;
import org.bukkit.inventory.Merchant;
import org.bukkit.inventory.MerchantRecipe;
import org.jetbrains.annotations.Nullable;

/**
 * A merchant a plugin made ({@code Server.createMerchant}, Paper's {@code CraftMerchantCustom}).
 * The server keeps its trades, so players trading with it use them up; once plugins drop the
 * merchant, the server forgets it after the last trading window on it closes.
 */
public final class HostMerchant implements Merchant {
    private static final Cleaner CLEANER = Cleaner.create();
    private static final Map<Long, WeakReference<HostMerchant>> KNOWN = new ConcurrentHashMap<>();

    private final CraftServer server;
    private final long id;

    private HostMerchant(final CraftServer server, final long id) {
        this.server = server;
        this.id = id;
    }

    /** A new merchant; without a title the window says "Villager", as in Paper. */
    public static Merchant create(final CraftServer server, final @Nullable Component title) {
        final Map<String, Object> created = Values.map(server.call("merchant.create", Values.of("title", title == null ? null : Components.toJson(title))));
        final long id = Values.i64(created, "id");
        final HostMerchant merchant = new HostMerchant(server, id);
        KNOWN.put(id, new WeakReference<>(merchant));
        CLEANER.register(merchant, () -> {
            KNOWN.remove(id);
            server.send("merchant.release", Values.of("merchant", id));
        });
        return merchant;
    }

    /** The merchant with a server id, if plugins still hold it. */
    public static @Nullable Merchant of(final long id) {
        final WeakReference<HostMerchant> ref = KNOWN.get(id);
        return ref == null ? null : ref.get();
    }

    /** Forget all merchants (the plugin host is going away). */
    public static void clear() {
        KNOWN.clear();
    }

    public long id() {
        return this.id;
    }

    private Map<String, Object> args(final Object... kv) {
        final Map<String, Object> args = Values.of(kv);
        args.put("merchant", this.id);
        return args;
    }

    @Override
    public List<MerchantRecipe> getRecipes() {
        return Merchants.recipes(this.server.call("merchant.recipes", this.args()));
    }

    @Override
    public void setRecipes(final List<MerchantRecipe> recipes) {
        this.server.call("merchant.set_recipes", this.args("recipes", Merchants.values(recipes)));
    }

    @Override
    public MerchantRecipe getRecipe(final int i) {
        return this.getRecipes().get(i);
    }

    @Override
    public void setRecipe(final int i, final MerchantRecipe recipe) {
        Objects.checkIndex(i, this.getRecipeCount());
        this.server.call("merchant.set_recipe", this.args("index", i, "recipe", Merchants.value(recipe)));
    }

    @Override
    public int getRecipeCount() {
        return Values.list(this.server.call("merchant.recipes", this.args())).size();
    }

    @Override
    public boolean isTrading() {
        return this.getTrader() != null;
    }

    @Override
    public @Nullable HumanEntity getTrader() {
        final Object trader = this.server.call("merchant.trader", this.args());
        return trader instanceof String uuid ? this.server.player(UUID.fromString(uuid)) : null;
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostMerchant other && other.id == this.id;
    }

    @Override
    public int hashCode() {
        return Long.hashCode(this.id);
    }

    @Override
    public String toString() {
        return "CraftMerchantCustom{" + this.id + "}";
    }
}
