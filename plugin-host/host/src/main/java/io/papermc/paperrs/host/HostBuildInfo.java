package io.papermc.paperrs.host;

import io.papermc.paperrs.host.stub.AbstractServerBuildInfo;
import java.time.Instant;
import java.util.Optional;
import java.util.OptionalInt;
import net.kyori.adventure.key.Key;

/** {@code ServerBuildInfo} for Paper-RS. Plugins that check for Paper are told it is compatible. */
public final class HostBuildInfo extends AbstractServerBuildInfo {
    public static final Key BRAND_ID = Key.key("paper-rs", "paper-rs");

    @Override
    public Key brandId() {
        return BRAND_ID;
    }

    @Override
    public boolean isBrandCompatible(final Key brandId) {
        return brandId.equals(BRAND_ID) || brandId.equals(BRAND_PAPER_ID);
    }

    @Override
    public String brandName() {
        return "Paper-RS";
    }

    @Override
    public String minecraftVersionId() {
        return HostVersions.MINECRAFT_VERSION;
    }

    @Override
    public String minecraftVersionName() {
        return HostVersions.MINECRAFT_VERSION;
    }

    @Override
    public OptionalInt buildNumber() {
        return OptionalInt.empty();
    }

    @Override
    public Instant buildTime() {
        return Instant.EPOCH;
    }

    @Override
    public Optional<String> gitBranch() {
        return Optional.empty();
    }

    @Override
    public Optional<String> gitCommit() {
        return Optional.empty();
    }

    @Override
    public String asString(final StringRepresentation representation) {
        final String simple = HostVersions.MINECRAFT_VERSION + "-DEV";
        return representation == StringRepresentation.VERSION_SIMPLE ? simple : simple + " (Paper-RS)";
    }
}
