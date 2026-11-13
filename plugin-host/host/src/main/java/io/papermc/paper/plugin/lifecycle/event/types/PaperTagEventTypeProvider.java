package io.papermc.paper.plugin.lifecycle.event.types;

import io.papermc.paper.plugin.bootstrap.BootstrapContext;
import io.papermc.paper.plugin.lifecycle.event.registrar.ReloadableRegistrarEvent;
import io.papermc.paper.registry.RegistryKey;
import io.papermc.paper.tag.PostFlattenTagRegistrar;
import io.papermc.paper.tag.PreFlattenTagRegistrar;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Tag lifecycle event types, one per registry.
 *
 * <p>Paper-RS replacement for Paper's class of the same name. Tags come from the Rust
 * server's data and are fixed, so these events are never fired: plugins can register
 * handlers (which must not fail, because {@code LifecycleEvents} creates these types in its
 * static initialiser), but the handlers do not run.
 */
public final class PaperTagEventTypeProvider implements TagEventTypeProvider {
    private final Map<RegistryKey<?>, Object> preFlatten = new ConcurrentHashMap<>();
    private final Map<RegistryKey<?>, Object> postFlatten = new ConcurrentHashMap<>();

    @Override
    @SuppressWarnings("unchecked")
    public <T> LifecycleEventType.Prioritizable<BootstrapContext, ReloadableRegistrarEvent<PreFlattenTagRegistrar<T>>> preFlatten(final RegistryKey<T> registryKey) {
        return (LifecycleEventType.Prioritizable<BootstrapContext, ReloadableRegistrarEvent<PreFlattenTagRegistrar<T>>>) this.preFlatten.computeIfAbsent(registryKey,
            key -> new PrioritizableLifecycleEventType.Simple<BootstrapContext, ReloadableRegistrarEvent<PreFlattenTagRegistrar<T>>>("tags_pre_flatten / " + key.key().asString(), BootstrapContext.class));
    }

    @Override
    @SuppressWarnings("unchecked")
    public <T> LifecycleEventType.Prioritizable<BootstrapContext, ReloadableRegistrarEvent<PostFlattenTagRegistrar<T>>> postFlatten(final RegistryKey<T> registryKey) {
        return (LifecycleEventType.Prioritizable<BootstrapContext, ReloadableRegistrarEvent<PostFlattenTagRegistrar<T>>>) this.postFlatten.computeIfAbsent(registryKey,
            key -> new PrioritizableLifecycleEventType.Simple<BootstrapContext, ReloadableRegistrarEvent<PostFlattenTagRegistrar<T>>>("tags_post_flatten / " + key.key().asString(), BootstrapContext.class));
    }
}
