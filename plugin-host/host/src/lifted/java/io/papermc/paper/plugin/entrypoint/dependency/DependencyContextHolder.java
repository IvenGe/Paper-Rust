// Copied from paper-server/src/main/java/io/papermc/paper/plugin/entrypoint/dependency/DependencyContextHolder.java by plugin-host/host/tools/lift.py.
package io.papermc.paper.plugin.entrypoint.dependency;

import io.papermc.paper.plugin.provider.entrypoint.DependencyContext;

public interface DependencyContextHolder {

    void setContext(DependencyContext context);

}
