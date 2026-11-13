// Copied from paper-server/src/main/java/io/papermc/paper/plugin/lifecycle/event/PaperLifecycleEvent.java by plugin-host/host/tools/lift.py.
package io.papermc.paper.plugin.lifecycle.event;

public interface PaperLifecycleEvent extends LifecycleEvent {

    // called after all handlers have been run. Can be
    // used to invalid various contexts to plugins can't
    // try to re-use them by storing them from the event
    default void invalidate() {
    }
}
