package io.papermc.paper.plugin;

import io.papermc.paper.plugin.entrypoint.LaunchEntryPointHandler;
import io.papermc.paper.plugin.provider.source.DirectoryProviderSource;
import io.papermc.paper.plugin.provider.source.PluginFlagProviderSource;
import io.papermc.paper.plugin.provider.type.paper.PaperPluginParent;
import io.papermc.paper.plugin.provider.type.spigot.SpigotPluginProvider;
import io.papermc.paper.plugin.util.EntrypointUtil;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Set;
import java.util.TreeSet;
import java.util.logging.Logger;
import org.jetbrains.annotations.Nullable;

/**
 * Where plugins live, and the scan that registers them with the entrypoint handler.
 *
 * <p>Paper-RS replacement for Paper's class of the same name, which reads its directories
 * from the server's command line options and {@code bukkit.yml}. The lifted plugin loader
 * calls {@link #instance()}, {@link #pluginDirectoryPath()} and {@link #pluginUpdatePath()}.
 */
public final class PluginInitializerManager {
    private static final Logger LOGGER = Logger.getLogger("PluginInitializerManager");
    private static PluginInitializerManager impl;

    private final Path pluginDirectory;
    private final @Nullable Path updateDirectory;

    private PluginInitializerManager(final Path pluginDirectory, final @Nullable Path updateDirectory) {
        this.pluginDirectory = pluginDirectory;
        this.updateDirectory = updateDirectory;
    }

    public static PluginInitializerManager instance() {
        return impl;
    }

    public Path pluginDirectoryPath() {
        return this.pluginDirectory;
    }

    public @Nullable Path pluginUpdatePath() {
        return this.updateDirectory;
    }

    /**
     * Scans {@code pluginDirectory} (and {@code extraJars}) and registers every plugin found.
     * Paper plugins have their loader run and their class loader created here, as on Paper.
     */
    public static PluginInitializerManager load(final Path pluginDirectory, final String updateFolderName, final List<Path> extraJars) {
        Path update = null;
        if (!updateFolderName.isBlank()) {
            final Path resolved = pluginDirectory.resolve(updateFolderName);
            if (Files.isDirectory(resolved)) {
                update = resolved;
            }
        }
        impl = new PluginInitializerManager(pluginDirectory, update);

        LOGGER.info("Initializing plugins...");
        EntrypointUtil.registerProvidersFromSource(DirectoryProviderSource.INSTANCE, pluginDirectory);
        EntrypointUtil.registerProvidersFromSource(PluginFlagProviderSource.INSTANCE, extraJars);

        final Set<String> paperPluginNames = new TreeSet<>();
        final Set<String> legacyPluginNames = new TreeSet<>();
        LaunchEntryPointHandler.INSTANCE.getStorage().forEach((entrypoint, providerStorage) -> {
            providerStorage.getRegisteredProviders().forEach(provider -> {
                if (provider instanceof final SpigotPluginProvider legacy) {
                    legacyPluginNames.add(String.format("%s (%s)", legacy.getMeta().getName(), legacy.getMeta().getVersion()));
                } else if (provider instanceof final PaperPluginParent.PaperServerPluginProvider paper) {
                    paperPluginNames.add(String.format("%s (%s)", paper.getMeta().getName(), paper.getMeta().getVersion()));
                }
            });
        });
        final int total = paperPluginNames.size() + legacyPluginNames.size();
        LOGGER.info(String.format("Initialized %d plugin%s", total, total == 1 ? "" : "s"));
        if (!paperPluginNames.isEmpty()) {
            LOGGER.info(String.format("Paper plugins (%d):%n - %s", paperPluginNames.size(), String.join(", ", paperPluginNames)));
        }
        if (!legacyPluginNames.isEmpty()) {
            LOGGER.info(String.format("Bukkit plugins (%d):%n - %s", legacyPluginNames.size(), String.join(", ", legacyPluginNames)));
        }
        return impl;
    }
}
