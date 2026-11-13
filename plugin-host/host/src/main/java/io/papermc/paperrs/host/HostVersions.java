package io.papermc.paperrs.host;

/** Version facts the host reports to plugins. */
public final class HostVersions {
    /** The Minecraft version the host and the Rust server implement. */
    public static final String MINECRAFT_VERSION = "26.3";
    public static final int PROTOCOL_VERSION = 777;
    public static final int DATA_VERSION = 5023;
    /** Matches Paper's default for console.has-all-permissions. */
    public static final boolean CONSOLE_HAS_ALL_PERMISSIONS = false;

    private HostVersions() {
    }
}
