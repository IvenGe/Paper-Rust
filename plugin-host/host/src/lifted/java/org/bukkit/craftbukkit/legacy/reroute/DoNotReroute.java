// Copied from paper-server/src/main/java/org/bukkit/craftbukkit/legacy/reroute/DoNotReroute.java by plugin-host/host/tools/lift.py.
package org.bukkit.craftbukkit.legacy.reroute;

import java.lang.annotation.ElementType;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.annotation.Target;

@Retention(RetentionPolicy.RUNTIME)
@Target(ElementType.METHOD)
public @interface DoNotReroute {
}
