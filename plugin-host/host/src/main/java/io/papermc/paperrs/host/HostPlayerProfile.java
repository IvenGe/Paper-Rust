package io.papermc.paperrs.host;

import com.destroystokyo.paper.profile.PlayerProfile;
import com.destroystokyo.paper.profile.ProfileProperty;
import io.papermc.paperrs.host.stub.AbstractPlayerProfile;
import java.util.Collection;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import org.jetbrains.annotations.Nullable;

/** A game profile: name, id and properties (the skin). Profiles are never looked up remotely. */
public final class HostPlayerProfile extends AbstractPlayerProfile {
    private @Nullable UUID id;
    private @Nullable String name;
    private final Map<String, ProfileProperty> properties = new LinkedHashMap<>();

    public HostPlayerProfile(final @Nullable UUID id, final @Nullable String name) {
        this.id = id;
        this.name = name;
    }

    @Override
    public @Nullable UUID getId() {
        return this.id;
    }

    @Override
    public @Nullable UUID getUniqueId() {
        return this.id;
    }

    @Override
    public @Nullable UUID setId(final @Nullable UUID id) {
        final UUID old = this.id;
        this.id = id;
        return old;
    }

    @Override
    public @Nullable String getName() {
        return this.name;
    }

    @Override
    public @Nullable String setName(final @Nullable String name) {
        final String old = this.name;
        this.name = name;
        return old;
    }

    @Override
    public Set<ProfileProperty> getProperties() {
        return Set.copyOf(this.properties.values());
    }

    @Override
    public boolean hasProperty(final String property) {
        return this.properties.containsKey(property);
    }

    @Override
    public void setProperty(final ProfileProperty property) {
        this.properties.put(property.getName(), property);
    }

    @Override
    public void setProperties(final Collection<ProfileProperty> properties) {
        properties.forEach(this::setProperty);
    }

    @Override
    public boolean removeProperty(final String property) {
        return this.properties.remove(property) != null;
    }

    @Override
    public void clearProperties() {
        this.properties.clear();
    }

    @Override
    public boolean isComplete() {
        return this.id != null && this.name != null;
    }

    @Override
    public boolean completeFromCache() {
        return this.isComplete();
    }

    @Override
    public boolean completeFromCache(final boolean onlineMode) {
        return this.isComplete();
    }

    @Override
    public boolean completeFromCache(final boolean lookupUUID, final boolean onlineMode) {
        return this.isComplete();
    }

    @Override
    public boolean complete(final boolean textures) {
        return this.isComplete();
    }

    @Override
    public boolean complete(final boolean textures, final boolean onlineMode) {
        return this.isComplete();
    }

    @Override
    public CompletableFuture<PlayerProfile> update() {
        return CompletableFuture.completedFuture(this.clone());
    }

    @Override
    public PlayerProfile clone() {
        final HostPlayerProfile copy = new HostPlayerProfile(this.id, this.name);
        copy.properties.putAll(this.properties);
        return copy;
    }

    @Override
    public Map<String, Object> serialize() {
        final Map<String, Object> out = new LinkedHashMap<>();
        if (this.id != null) {
            out.put("uniqueId", this.id.toString());
        }
        if (this.name != null) {
            out.put("name", this.name);
        }
        return out;
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostPlayerProfile p && java.util.Objects.equals(p.id, this.id) && java.util.Objects.equals(p.name, this.name);
    }

    @Override
    public int hashCode() {
        return java.util.Objects.hash(this.id, this.name);
    }

    @Override
    public String toString() {
        return "HostPlayerProfile{id=" + this.id + ", name=" + this.name + "}";
    }
}
