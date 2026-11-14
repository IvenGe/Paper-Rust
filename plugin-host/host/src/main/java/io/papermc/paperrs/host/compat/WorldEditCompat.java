package io.papermc.paperrs.host.compat;

import java.io.IOException;
import java.io.InputStream;
import java.lang.invoke.MethodHandles;
import java.util.HashMap;
import java.util.Map;
import java.util.logging.Level;
import java.util.logging.Logger;
import org.bukkit.plugin.Plugin;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.commons.ClassRemapper;
import org.objectweb.asm.commons.SimpleRemapper;

/**
 * WorldEdit on Paper-RS. WorldEdit only works through an adapter for the server's version,
 * and the adapters it ships use Minecraft's internal classes, which Paper-RS does not have.
 * WorldEdit lets the server name another adapter (the {@code worldedit.bukkit.adapter} system
 * property) that it loads with its own class loader; so before WorldEdit is enabled, the host
 * defines its adapter ({@code compat.worldedit}, compiled against WorldEdit's API) in that class
 * loader, in WorldEdit's package, and names it.
 */
public final class WorldEditCompat {
    private static final Logger LOGGER = Logger.getLogger("Minecraft");
    private static final String SOURCE = "io/papermc/paperrs/host/compat/worldedit/";
    private static final String TARGET = "com/sk89q/worldedit/bukkit/";
    /** The adapter's classes, each before the classes that need it. */
    private static final String[] CLASSES = {
        "PaperRsBlockMaterial", "PaperRsWorldNativeAccess$Chunk", "PaperRsWorldNativeAccess$State", "PaperRsWorldNativeAccess", "PaperRsAdapter",
    };
    private static final String PLUGIN_CLASS = "com.sk89q.worldedit.bukkit.WorldEditPlugin";

    private WorldEditCompat() {
    }

    /** Called before each plugin is enabled. */
    public static void beforeEnable(final Plugin plugin) {
        if (!PLUGIN_CLASS.equals(plugin.getClass().getName())) {
            return;
        }
        try {
            final Map<String, String> names = new HashMap<>();
            for (final String name : CLASSES) {
                names.put(SOURCE + name, TARGET + name);
            }
            final SimpleRemapper remapper = new SimpleRemapper(names);
            final MethodHandles.Lookup lookup = MethodHandles.privateLookupIn(plugin.getClass(), MethodHandles.lookup());
            for (final String name : CLASSES) {
                final ClassReader reader = new ClassReader(read(SOURCE + name + ".class"));
                final ClassWriter writer = new ClassWriter(0);
                reader.accept(new ClassRemapper(writer, remapper), 0);
                try {
                    lookup.defineClass(writer.toByteArray());
                } catch (LinkageError alreadyDefined) {
                    // The same class loader after a /reload of only this plugin: still there.
                }
            }
            System.setProperty("worldedit.bukkit.adapter", (TARGET + "PaperRsAdapter").replace('/', '.'));
        } catch (ReflectiveOperationException | IOException | RuntimeException e) {
            LOGGER.log(Level.WARNING, "Could not give WorldEdit the Paper-RS adapter; WorldEdit will not work", e);
        }
    }

    private static byte[] read(final String resource) throws IOException {
        try (InputStream in = WorldEditCompat.class.getClassLoader().getResourceAsStream(resource)) {
            if (in == null) {
                throw new IOException("missing " + resource);
            }
            return in.readAllBytes();
        }
    }
}
