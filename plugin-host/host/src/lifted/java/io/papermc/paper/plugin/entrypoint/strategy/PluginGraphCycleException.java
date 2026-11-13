// Copied from paper-server/src/main/java/io/papermc/paper/plugin/entrypoint/strategy/PluginGraphCycleException.java by plugin-host/host/tools/lift.py.
package io.papermc.paper.plugin.entrypoint.strategy;

import java.util.List;

/**
 * Indicates a dependency cycle within a provider loading sequence.
 */
public class PluginGraphCycleException extends RuntimeException {

    private final List<List<String>> cycles;

    public PluginGraphCycleException(List<List<String>> cycles) {
        this.cycles = cycles;
    }

    public List<List<String>> getCycles() {
        return this.cycles;
    }
}
