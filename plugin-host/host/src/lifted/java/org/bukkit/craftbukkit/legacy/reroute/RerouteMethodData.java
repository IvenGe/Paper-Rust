// Copied from paper-server/src/main/java/org/bukkit/craftbukkit/legacy/reroute/RerouteMethodData.java by plugin-host/host/tools/lift.py.
package org.bukkit.craftbukkit.legacy.reroute;

import java.util.List;
import org.jetbrains.annotations.Nullable;
import org.objectweb.asm.Type;

public record RerouteMethodData(String methodKey, Type sourceDesc, Type sourceOwner, String sourceName,
                                boolean staticReroute, Type targetType, String targetOwner, String targetName,
                                List<RerouteArgument> arguments, RerouteReturn rerouteReturn, boolean isInBukkit,
                                @Nullable RequirePluginVersionData requiredPluginVersion) {
}
