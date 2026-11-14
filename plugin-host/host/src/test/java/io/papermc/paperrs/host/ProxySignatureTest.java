package io.papermc.paperrs.host;

import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import io.papermc.paperrs.host.entity.HostEntity;
import io.papermc.paperrs.host.entity.HostEquipment;
import io.papermc.paperrs.host.inventory.HostInventory;
import io.papermc.paperrs.host.inventory.HostInventoryView;
import io.papermc.paperrs.host.item.HostItemMeta;
import io.papermc.paperrs.host.item.MetaKinds;
import io.papermc.paperrs.host.scoreboard.HostScoreboards;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Set;
import org.junit.jupiter.api.Test;

/**
 * The host implements API interfaces through dynamic proxies ({@link HostProxies}), which
 * match implementation methods by name and parameter types at run time. The compiler cannot
 * check those, so this test does: every public method of an implementation class whose name
 * the API interfaces use must match one of their methods exactly, with a compatible return
 * type. Otherwise a typo would silently turn an implemented method into an
 * "unsupported" one.
 */
class ProxySignatureTest {
    private static final Set<String> HELPERS = Set.of("bind", "self", "equals", "hashCode", "toString");
    /**
     * Methods whose implementation returns a supertype on purpose: the object returned is of
     * the type each interface wants ({@code getHolder} of a player inventory is the player),
     * or the method refuses the interfaces it cannot serve ({@code Item#getHealth}).
     */
    private static final Set<String> COVARIANT = Set.of("getHolder", "getHealth", "getInventory", "getSnapshotInventory", "getBlockInventory");

    private static void check(final Class<?> impl, final Class<?>... interfaces) {
        final List<String> problems = new ArrayList<>();
        final List<Method> api = new ArrayList<>();
        for (Class<?> iface : interfaces) {
            api.addAll(Arrays.asList(iface.getMethods()));
        }
        for (Method m : impl.getDeclaredMethods()) {
            if (!Modifier.isPublic(m.getModifiers()) || Modifier.isStatic(m.getModifiers()) || m.isSynthetic() || HELPERS.contains(m.getName())) {
                continue;
            }
            final List<Method> named = api.stream().filter(a -> a.getName().equals(m.getName())).toList();
            if (named.isEmpty()) {
                continue;
            }
            final List<Method> exact = named.stream().filter(a -> Arrays.equals(a.getParameterTypes(), m.getParameterTypes())).toList();
            if (exact.isEmpty()) {
                problems.add(impl.getSimpleName() + "#" + m.getName() + Arrays.toString(m.getParameterTypes()) + " matches no API signature");
                continue;
            }
            for (Method a : exact) {
                final Class<?> want = a.getReturnType();
                final Class<?> got = m.getReturnType();
                final boolean ok = want == got || want == void.class && got == void.class
                    || !want.isPrimitive() && (want.isAssignableFrom(got) || got == Object.class)
                    || COVARIANT.contains(m.getName());
                if (!ok) {
                    problems.add(impl.getSimpleName() + "#" + m.getName() + " returns " + got.getName() + ", " + a.getDeclaringClass().getSimpleName() + " wants " + want.getName());
                }
            }
        }
        if (!problems.isEmpty()) {
            fail(String.join("\n", problems));
        }
    }

    @Test
    void inventoriesMatchTheApi() {
        check(HostInventory.class, org.bukkit.inventory.PlayerInventory.class, org.bukkit.inventory.FurnaceInventory.class,
            org.bukkit.inventory.CraftingInventory.class, org.bukkit.inventory.DoubleChestInventory.class);
        check(HostInventoryView.class, org.bukkit.inventory.InventoryView.class);
        check(HostEquipment.class, org.bukkit.inventory.EntityEquipment.class);
    }

    @Test
    void blockStatesMatchTheApi() {
        check(io.papermc.paperrs.host.world.HostBlockState.class, org.bukkit.block.Chest.class, org.bukkit.block.Furnace.class, org.bukkit.block.Barrel.class);
    }

    @Test
    void itemMetasMatchTheApi() {
        final List<Class<?>> kinds = new ArrayList<>();
        for (Class<?> kind : MetaKinds.class.getDeclaredClasses()) {
            if (kind.isInterface()) {
                kinds.add(kind);
            }
        }
        check(HostItemMeta.class, kinds.toArray(Class<?>[]::new));
    }

    @Test
    void entitiesMatchTheApi() {
        check(HostEntity.class, org.bukkit.entity.Zombie.class, org.bukkit.entity.Item.class, org.bukkit.entity.ExperienceOrb.class,
            org.bukkit.entity.Arrow.class, org.bukkit.entity.TNTPrimed.class, org.bukkit.entity.FallingBlock.class, org.bukkit.entity.Sheep.class,
            org.bukkit.entity.Wolf.class, org.bukkit.entity.Horse.class, org.bukkit.entity.Pig.class);
    }

    @Test
    void scoreboardsMatchTheApi() {
        check(HostScoreboards.Board.class, org.bukkit.scoreboard.Scoreboard.class);
        check(HostScoreboards.HostObjective.class, org.bukkit.scoreboard.Objective.class);
        check(HostScoreboards.HostScore.class, org.bukkit.scoreboard.Score.class);
        check(HostScoreboards.HostTeam.class, org.bukkit.scoreboard.Team.class);
        check(HostScoreboards.HostCriteria.class, org.bukkit.scoreboard.Criteria.class);
        check(HostScoreboards.Manager.class, org.bukkit.scoreboard.ScoreboardManager.class);
    }

    @Test
    void everyEntityInterfaceMakesAProxy() {
        // Entities are proxies of their type's interface; that must work for every type.
        for (org.bukkit.entity.EntityType type : org.bukkit.entity.EntityType.values()) {
            final Class<?> iface = type.getEntityClass();
            if (iface != null && iface.isInterface()) {
                final Object proxy = Proxy.newProxyInstance(HostProxies.class.getClassLoader(), new Class<?>[] {iface}, (p, m, a) -> null);
                assertTrue(iface.isInstance(proxy), iface.getName());
            }
        }
    }

    @Test
    void everyMetaKindMakesAProxy() {
        // Proxy creation fails if two interfaces declare a method with incompatible returns.
        for (Class<?> kind : MetaKinds.class.getDeclaredClasses()) {
            if (kind.isInterface()) {
                final Object proxy = Proxy.newProxyInstance(HostProxies.class.getClassLoader(), new Class<?>[] {kind}, (p, m, a) -> null);
                assertTrue(kind.isInstance(proxy), kind.getName());
            }
        }
    }
}
