package io.papermc.paperrs.host;

import io.papermc.paper.InternalAPIBridge;
import io.papermc.paper.ServerBuildInfo;
import io.papermc.paperrs.host.bridge.Bridge;
import io.papermc.paperrs.host.bridge.Values;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.logging.Level;
import java.util.logging.Logger;

/**
 * Entry point of the plugin host. The Rust server starts it as
 * {@code java -jar paper-rs-plugin-host.jar --connect 127.0.0.1:PORT --root DIR --plugins DIR}
 * with the bridge token in {@code PAPER_RS_BRIDGE_TOKEN}.
 */
public final class Main {
    /** Bridge protocol version; must match {@code paper_bridge::PROTOCOL_VERSION}. */
    public static final long BRIDGE_PROTOCOL = 1;

    private Main() {
    }

    public static void main(final String[] args) {
        String connect = null;
        Path root = Path.of(".");
        Path plugins = Path.of("plugins");
        final List<Path> extra = new ArrayList<>();
        for (int i = 0; i < args.length; i++) {
            switch (args[i]) {
                case "--connect" -> connect = args[++i];
                case "--root" -> root = Path.of(args[++i]);
                case "--plugins" -> plugins = Path.of(args[++i]);
                case "--add-plugin" -> extra.add(Path.of(args[++i]));
                default -> {
                    System.err.println("Unknown argument " + args[i]);
                    System.exit(2);
                }
            }
        }
        HostLogging.install();
        final Logger logger = Logger.getLogger("PluginHost");
        final String token = System.getenv("PAPER_RS_BRIDGE_TOKEN");
        if (connect == null || token == null) {
            logger.severe("The plugin host is started by the Paper-RS server (missing --connect or PAPER_RS_BRIDGE_TOKEN)");
            System.exit(2);
        }
        Thread.currentThread().setName("Server thread");
        int exit = 0;
        try {
            final int colon = connect.lastIndexOf(':');
            final Socket socket = new Socket();
            socket.connect(new InetSocketAddress(connect.substring(0, colon), Integer.parseInt(connect.substring(colon + 1))), 10_000);
            final Bridge bridge = new Bridge(socket);
            final Map<String, Object> welcome = bridge.handshake(Values.of("token", token, "protocol", BRIDGE_PROTOCOL, "host", "paper-rs-plugin-host"));
            // These two cache their service lookup (made through the context class loader) for
            // the life of the JVM, failures included: resolve them here, first.
            InternalAPIBridge.get();
            ServerBuildInfo.buildInfo();
            final Host host = new Host(bridge, welcome, root.toAbsolutePath(), plugins.toAbsolutePath(), extra);
            bridge.start(host);
            bridge.runMainLoop();
            host.onDisconnected();
        } catch (Throwable t) {
            logger.log(Level.SEVERE, "The plugin host failed", t);
            exit = 1;
        }
        System.out.flush();
        // Plugin threads must not keep the process alive once the server is gone.
        Runtime.getRuntime().halt(exit);
    }
}
