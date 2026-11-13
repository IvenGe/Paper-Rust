package io.papermc.paperrs.host;

import java.lang.reflect.InvocationHandler;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.lang.reflect.Proxy;
import java.util.Arrays;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;
import org.jetbrains.annotations.Nullable;

/**
 * API objects whose exact interface depends on what they represent (entities, block states,
 * item metas, inventories): a dynamic proxy implements the specific API interfaces (for
 * example {@code Zombie} or {@code Chest}) and forwards each call to a plain implementation
 * object with a public method of the same name and parameter types.
 *
 * <p>Calls without such a method run the interface's default method if it has one, and
 * otherwise throw {@link Unsupported} naming the method, so a plugin that reaches a missing
 * feature fails with a clear message, as with the generated stub classes. {@code equals},
 * {@code hashCode} and {@code toString} go to the implementation object.
 */
public final class HostProxies {
    /** Implementation methods by (implementation class, interface method), or none. */
    private static final Map<Class<?>, Map<Method, Optional<Method>>> TARGETS = new ConcurrentHashMap<>();

    private HostProxies() {
    }

    /** An implementation object that knows the proxy standing for it. */
    public interface Backed {
        /** Called once with the proxy, before the proxy is handed out. */
        void bind(Object self);
    }

    /** Creates a proxy implementing {@code interfaces} that forwards to {@code impl}. */
    @SuppressWarnings("unchecked")
    public static <T> T create(final Object impl, final Class<?>... interfaces) {
        final Class<?>[] distinct = Arrays.stream(interfaces).distinct().toArray(Class<?>[]::new);
        final Object proxy = Proxy.newProxyInstance(HostProxies.class.getClassLoader(), distinct, new Handler(impl));
        if (impl instanceof Backed backed) {
            backed.bind(proxy);
        }
        return (T) proxy;
    }

    /** The implementation behind a proxy made here, or {@code null}. */
    public static @Nullable Object implOf(final @Nullable Object object) {
        if (object != null && Proxy.isProxyClass(object.getClass()) && Proxy.getInvocationHandler(object) instanceof Handler handler) {
            return handler.impl;
        }
        return null;
    }

    /** The implementation behind a proxy made here if it has the given type. */
    public static <T> @Nullable T implOf(final @Nullable Object object, final Class<T> type) {
        final Object impl = implOf(object);
        return type.isInstance(impl) ? type.cast(impl) : null;
    }

    private static Optional<Method> target(final Class<?> implClass, final Method method) {
        return TARGETS.computeIfAbsent(implClass, c -> new ConcurrentHashMap<>()).computeIfAbsent(method, m -> {
            try {
                final Method found = implClass.getMethod(m.getName(), m.getParameterTypes());
                if (found.getDeclaringClass() == Object.class && !isObjectMethod(m)) {
                    return Optional.empty();
                }
                if (!Modifier.isPublic(found.getDeclaringClass().getModifiers())) {
                    found.setAccessible(true);
                }
                return Optional.of(found);
            } catch (NoSuchMethodException e) {
                return Optional.empty();
            }
        });
    }

    private static boolean isObjectMethod(final Method m) {
        return switch (m.getName()) {
            case "equals" -> m.getParameterCount() == 1;
            case "hashCode", "toString" -> m.getParameterCount() == 0;
            default -> false;
        };
    }

    private static final class Handler implements InvocationHandler {
        private final Object impl;

        Handler(final Object impl) {
            this.impl = impl;
        }

        @Override
        public Object invoke(final Object proxy, final Method method, final Object[] args) throws Throwable {
            if (method.getName().equals("equals") && method.getParameterCount() == 1) {
                // Compare implementations, so two proxies for the same thing are equal.
                final Object other = args[0];
                final Object otherImpl = implOf(other);
                return this.impl.equals(otherImpl != null ? otherImpl : other);
            }
            final Optional<Method> target = target(this.impl.getClass(), method);
            if (target.isPresent()) {
                try {
                    return target.get().invoke(this.impl, args);
                } catch (InvocationTargetException e) {
                    throw e.getCause();
                }
            }
            if (method.isDefault()) {
                return InvocationHandler.invokeDefault(proxy, method, args);
            }
            throw Unsupported.method(ownerName(method.getDeclaringClass()) + "#" + method.getName());
        }
    }

    private static String ownerName(final Class<?> type) {
        final StringBuilder name = new StringBuilder(type.getSimpleName());
        for (Class<?> outer = type.getEnclosingClass(); outer != null; outer = outer.getEnclosingClass()) {
            name.insert(0, outer.getSimpleName() + ".");
        }
        return name.toString();
    }
}
