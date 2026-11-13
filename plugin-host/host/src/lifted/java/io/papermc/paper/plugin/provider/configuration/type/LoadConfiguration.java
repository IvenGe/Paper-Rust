// Copied from paper-server/src/main/java/io/papermc/paper/plugin/provider/configuration/type/LoadConfiguration.java by plugin-host/host/tools/lift.py.
package io.papermc.paper.plugin.provider.configuration.type;

import org.spongepowered.configurate.objectmapping.ConfigSerializable;
import org.spongepowered.configurate.objectmapping.meta.Required;

@ConfigSerializable
public record LoadConfiguration(
    @Required String name,
    boolean bootstrap
) {
}
