// Copied from paper-server/src/main/java/io/papermc/paper/plugin/provider/ProviderStatusHolder.java by plugin-host/host/tools/lift.py.
package io.papermc.paper.plugin.provider;

/**
 * This is used to mark that a plugin provider is able to hold a status for the /plugins command.
 */
public interface ProviderStatusHolder {

    ProviderStatus getLastProvidedStatus();

    void setStatus(ProviderStatus status);
}
