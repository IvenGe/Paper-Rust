package io.papermc.paperrs.host.entity;

import com.destroystokyo.paper.profile.PlayerProfile;
import com.destroystokyo.paper.profile.ProfileProperty;
import io.papermc.paper.entity.TeleportFlag;
import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.HostPlayerProfile;
import io.papermc.paperrs.host.bridge.Values;
import io.papermc.paperrs.host.stub.AbstractPlayer;
import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import net.kyori.adventure.chat.ChatType;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.title.Title;
import net.kyori.adventure.title.TitlePart;
import org.bukkit.GameMode;
import org.bukkit.Location;
import org.bukkit.Server;
import org.bukkit.World;
import org.bukkit.block.BlockFace;
import org.bukkit.craftbukkit.CraftServer;
import org.bukkit.entity.Entity;
import org.bukkit.entity.EntityType;
import org.bukkit.entity.Player;
import org.bukkit.event.player.PlayerKickEvent;
import org.bukkit.event.player.PlayerTeleportEvent;
import org.bukkit.metadata.MetadataValue;
import org.bukkit.permissions.PermissibleBase;
import org.bukkit.permissions.Permission;
import org.bukkit.permissions.PermissionAttachment;
import org.bukkit.permissions.PermissionAttachmentInfo;
import org.bukkit.plugin.Plugin;
import org.bukkit.plugin.messaging.StandardMessenger;
import org.bukkit.util.Vector;
import org.jetbrains.annotations.Nullable;

/**
 * A connected player.
 *
 * <p>The host keeps a copy of the player's state, which the Rust server refreshes before every
 * call into plugin code, so reads are local. Changes are sent to the server, which applies
 * them and answers with the new state.
 */
public final class HostPlayer extends AbstractPlayer {
    private final CraftServer server;
    private final UUID uuid;
    private final String name;
    private final int entityId;
    private final PermissibleBase perm;
    private final HostPlayerProfile profile;
    private volatile Map<String, Object> state;
    private volatile boolean online = true;
    private @Nullable Component displayName;
    private org.bukkit.inventory.@Nullable PlayerInventory inventory;
    private org.bukkit.inventory.@Nullable Inventory enderChest;
    /** The inventory a plugin opened for this player, kept alive while the window is open. */
    private org.bukkit.inventory.@Nullable Inventory openTop;

    public HostPlayer(final CraftServer server, final Map<String, Object> state) {
        this.server = server;
        this.state = state;
        this.uuid = Values.uuid(state, "uuid");
        this.name = Values.str(state, "name");
        this.entityId = Values.i32(state, "entity_id", 0);
        this.profile = new HostPlayerProfile(this.uuid, this.name);
        for (Object p : Values.list(state.get("properties"))) {
            final Map<String, Object> prop = Values.map(p);
            this.profile.setProperty(new ProfileProperty(Values.str(prop, "name"), Values.str(prop, "value"), (String) prop.get("signature")));
        }
        this.displayName = Components.fromJson(state.get("display_name"));
        this.perm = new PermissibleBase(this);
    }

    /** Replaces the cached state with one sent by the server. */
    public void updateState(final Map<String, Object> state) {
        final boolean opChanged = Values.i64(state, "op_level", 0) != Values.i64(this.state, "op_level", 0);
        this.state = state;
        if (opChanged) {
            this.perm.recalculatePermissions();
        }
    }

    public void markOffline() {
        this.online = false;
    }

    private Map<String, Object> args(final Object... kv) {
        final Map<String, Object> args = Values.of(kv);
        args.put("uuid", this.uuid.toString());
        return args;
    }

    private void refresh(final Object state) {
        if (state instanceof Map<?, ?>) {
            this.updateState(Values.map(state));
        }
    }

    // --- identity ---------------------------------------------------------------------------

    @Override
    public String getName() {
        return this.name;
    }

    @Override
    public Component name() {
        return Component.text(this.name);
    }

    @Override
    public UUID getUniqueId() {
        return this.uuid;
    }

    // --- attributes ------------------------------------------------------------------------

    @Override
    public org.bukkit.attribute.@org.jetbrains.annotations.Nullable AttributeInstance getAttribute(final org.bukkit.attribute.Attribute attribute) {
        return HostAttributeInstance.of(this.server, this.entityId, attribute, () -> this.refresh(this.server.call("player.state", this.args())));
    }

    @Override
    public void registerAttribute(final org.bukkit.attribute.Attribute attribute) {
        HostAttributeInstance.register(this.server, this.entityId, attribute);
    }

    // --- persistent data ------------------------------------------------------------------------

    @Override
    public org.bukkit.persistence.PersistentDataContainer getPersistentDataContainer() {
        return Entities.data(this.server, Values.of("player", this.getUniqueId().toString()), this.getUniqueId());
    }

    // --- vehicles ------------------------------------------------------------------------------

    @Override
    public @Nullable Entity getVehicle() {
        this.refresh(this.server.call("player.state", this.args()));
        return this.state.get("vehicle") instanceof Map<?, ?> vehicle ? Entities.of(this.server, Values.map(vehicle)) : null;
    }

    @Override
    public boolean isInsideVehicle() {
        return this.getVehicle() != null;
    }

    @Override
    public boolean leaveVehicle() {
        return Boolean.TRUE.equals(this.server.call("entity.leave_vehicle", Values.of("rider", Values.of("player", this.getUniqueId().toString()))));
    }

    /** Nothing rides players on this server. */
    @Override
    public List<Entity> getPassengers() {
        return List.of();
    }

    @Override
    public boolean isEmpty() {
        return true;
    }

    @Override
    public boolean eject() {
        return false;
    }

    @Override
    public int getEntityId() {
        return this.entityId;
    }

    @Override
    public EntityType getType() {
        return EntityType.PLAYER;
    }

    @Override
    public PlayerProfile getPlayerProfile() {
        return this.profile.clone();
    }

    @Override
    public Server getServer() {
        return this.server;
    }

    @Override
    public Component displayName() {
        return this.displayName == null ? Component.text(this.name) : this.displayName;
    }

    @Override
    public void displayName(final @Nullable Component displayName) {
        this.displayName = displayName;
        this.server.send("player.set_display_name", this.args("name", Components.toJson(displayName)));
    }

    @Override
    public String getDisplayName() {
        return Components.toLegacy(this.displayName());
    }

    @Override
    public void setDisplayName(final @Nullable String name) {
        this.displayName(name == null ? null : Components.legacy(name));
    }

    @Override
    public Component playerListName() {
        final Component name = Components.fromJson(this.state.get("list_name"));
        return name == null ? Component.text(this.name) : name;
    }

    @Override
    public void playerListName(final @Nullable Component name) {
        this.server.send("player.set_list_name", this.args("name", Components.toJson(name)));
        this.state.put("list_name", Components.toJson(name));
    }

    @Override
    public String getPlayerListName() {
        return Components.toLegacy(this.playerListName());
    }

    @Override
    public void setPlayerListName(final @Nullable String name) {
        this.playerListName(name == null ? null : Components.legacy(name));
    }

    // --- connection -------------------------------------------------------------------------

    @Override
    public boolean isOnline() {
        return this.online;
    }

    @Override
    public boolean isConnected() {
        return this.online;
    }

    @Override
    public boolean isValid() {
        return this.online && !this.isDead();
    }

    @Override
    public @Nullable InetSocketAddress getAddress() {
        return parseAddress(Values.str(this.state, "address", null));
    }

    @Override
    public @Nullable InetSocketAddress getVirtualHost() {
        final String host = Values.str(this.state, "virtual_host", null);
        return host == null || host.isEmpty() ? null : InetSocketAddress.createUnresolved(host, this.server.getPort());
    }

    static @Nullable InetSocketAddress parseAddress(final @Nullable String address) {
        if (address == null) {
            return null;
        }
        final int colon = address.lastIndexOf(':');
        try {
            final String host = address.substring(0, colon).replace("[", "").replace("]", "");
            return new InetSocketAddress(java.net.InetAddress.getByName(host), Integer.parseInt(address.substring(colon + 1)));
        } catch (Exception e) {
            return null;
        }
    }

    @Override
    public int getPing() {
        return Values.i32(this.state, "ping", 0);
    }

    @Override
    public @Nullable String getClientBrandName() {
        return Values.str(this.state, "brand", null);
    }

    @Override
    public Locale locale() {
        return Locale.forLanguageTag(this.getLocale().replace('_', '-'));
    }

    @Override
    public String getLocale() {
        return Values.str(this.state, "locale", "en_us");
    }

    @Override
    public int getClientViewDistance() {
        return Values.i32(this.state, "view_distance", 10);
    }

    @Override
    public int getViewDistance() {
        return Values.i32(this.state, "view_distance", 10);
    }

    @Override
    public void kick(final @Nullable Component message, final PlayerKickEvent.Cause cause) {
        this.server.send("player.kick", this.args("reason", Components.toJson(message == null ? Component.translatable("multiplayer.disconnect.kicked") : message)));
    }

    @Override
    public void kickPlayer(final @Nullable String message) {
        this.kick(message == null ? null : Components.legacy(message), PlayerKickEvent.Cause.PLUGIN);
    }

    // --- messages ---------------------------------------------------------------------------

    @Override
    public void sendMessage(final Component message) {
        this.server.send("player.send_message", this.args("message", Components.toJson(message)));
    }

    @Override
    public void sendMessage(final Component message, final ChatType.Bound boundChatType) {
        this.sendMessage(message);
    }

    @Override
    public void sendMessage(final String message) {
        this.sendMessage(Components.legacy(message));
    }

    @Override
    public void sendMessage(final String... messages) {
        for (String message : messages) {
            this.sendMessage(message);
        }
    }

    @Override
    public void sendMessage(final @Nullable UUID sender, final String message) {
        this.sendMessage(message);
    }

    @Override
    public void sendMessage(final @Nullable UUID sender, final String... messages) {
        this.sendMessage(messages);
    }

    @Override
    public void sendRawMessage(final String message) {
        this.sendMessage(Component.text(message));
    }

    @Override
    public void sendRawMessage(final @Nullable UUID sender, final String message) {
        this.sendRawMessage(message);
    }

    // --- Adventure: Audience's default methods do nothing; each one works or throws ---------

    @Override
    public void sendMessage(final net.kyori.adventure.text.ComponentLike message) {
        this.sendMessage(message.asComponent());
    }

    @Override
    public void sendMessage(final net.kyori.adventure.text.ComponentLike message, final ChatType.Bound boundChatType) {
        this.sendMessage(message.asComponent(), boundChatType);
    }

    @Override
    public void sendMessage(final net.kyori.adventure.chat.SignedMessage signedMessage, final ChatType.Bound boundChatType) {
        throw io.papermc.paperrs.host.Unsupported.method("Player#sendMessage(SignedMessage, ChatType.Bound)");
    }

    @Override
    public void deleteMessage(final net.kyori.adventure.chat.SignedMessage signedMessage) {
        throw io.papermc.paperrs.host.Unsupported.method("Player#deleteMessage");
    }

    @Override
    public void deleteMessage(final net.kyori.adventure.chat.SignedMessage.Signature signature) {
        throw io.papermc.paperrs.host.Unsupported.method("Player#deleteMessage");
    }

    @Override
    public void sendActionBar(final net.kyori.adventure.text.ComponentLike message) {
        this.sendActionBar(message.asComponent());
    }

    private Component listHeader = Component.empty();
    private Component listFooter = Component.empty();

    @Override
    public void sendPlayerListHeader(final net.kyori.adventure.text.ComponentLike header) {
        this.sendPlayerListHeaderAndFooter(header.asComponent(), this.listFooter);
    }

    @Override
    public void sendPlayerListHeader(final Component header) {
        this.sendPlayerListHeaderAndFooter(header, this.listFooter);
    }

    @Override
    public void sendPlayerListFooter(final net.kyori.adventure.text.ComponentLike footer) {
        this.sendPlayerListHeaderAndFooter(this.listHeader, footer.asComponent());
    }

    @Override
    public void sendPlayerListFooter(final Component footer) {
        this.sendPlayerListHeaderAndFooter(this.listHeader, footer);
    }

    @Override
    public void sendPlayerListHeaderAndFooter(final net.kyori.adventure.text.ComponentLike header, final net.kyori.adventure.text.ComponentLike footer) {
        this.sendPlayerListHeaderAndFooter(header.asComponent(), footer.asComponent());
    }

    @Override
    public void showBossBar(final net.kyori.adventure.bossbar.BossBar bar) {
        io.papermc.paperrs.host.bossbar.BossBars.show(this.server, this, bar);
    }

    @Override
    public void hideBossBar(final net.kyori.adventure.bossbar.BossBar bar) {
        io.papermc.paperrs.host.bossbar.BossBars.hide(this.server, this, bar);
    }

    @Override
    public Iterable<? extends net.kyori.adventure.bossbar.BossBar> activeBossBars() {
        return io.papermc.paperrs.host.bossbar.BossBars.shownTo(this);
    }

    @Override
    public void playSound(final net.kyori.adventure.sound.Sound sound, final net.kyori.adventure.sound.Sound.Emitter emitter) {
        if (emitter == net.kyori.adventure.sound.Sound.Emitter.self()) {
            this.playSound(sound);
        } else if (emitter instanceof Entity entity) {
            final Location at = entity.getLocation();
            this.playSound(sound, at.getX(), at.getY(), at.getZ());
        } else {
            throw io.papermc.paperrs.host.Unsupported.method("Player#playSound(Sound, Sound.Emitter) with emitter " + emitter);
        }
    }

    @Override
    public void stopSound(final net.kyori.adventure.sound.Sound sound) {
        this.stopSound(net.kyori.adventure.sound.SoundStop.namedOnSource(sound.name(), sound.source()));
    }

    @Override
    public void stopSound(final net.kyori.adventure.sound.SoundStop stop) {
        final Map<String, Object> args = this.args();
        if (stop.sound() != null) {
            args.put("sound", stop.sound().asString());
        }
        if (stop.source() != null) {
            args.put("category", stop.source().ordinal());
        }
        this.server.send("player.stop_sound", args);
    }

    // --- fake blocks ---------------------------------------------------------------------------

    @Override
    public void sendBlockChange(final org.bukkit.Location loc, final org.bukkit.block.data.BlockData block) {
        this.server.send("player.block_change", this.args("x", loc.getBlockX(), "y", loc.getBlockY(), "z", loc.getBlockZ(),
            "state", io.papermc.paperrs.host.world.HostBlockData.impl(block).getAsString(false)));
    }

    @Override
    public void sendBlockChanges(final java.util.Collection<org.bukkit.block.BlockState> blocks) {
        for (final org.bukkit.block.BlockState state : blocks) {
            this.sendBlockChange(state.getLocation(), state.getBlockData());
        }
    }

    @Override
    public void sendMultiBlockChange(final Map<? extends io.papermc.paper.math.Position, org.bukkit.block.data.BlockData> blocks) {
        blocks.forEach((pos, data) -> this.server.send("player.block_change", this.args("x", pos.blockX(), "y", pos.blockY(), "z", pos.blockZ(),
            "state", io.papermc.paperrs.host.world.HostBlockData.impl(data).getAsString(false))));
    }

    @Override
    public void stopAllSounds() {
        this.server.send("player.stop_sound", this.args());
    }

    // --- books ---------------------------------------------------------------------------------

    @Override
    public void openBook(final org.bukkit.inventory.ItemStack book) {
        if (book == null || book.getType() != org.bukkit.Material.WRITTEN_BOOK) {
            throw new IllegalArgumentException("Book must be Material.WRITTEN_BOOK");
        }
        this.server.send("player.open_book", this.args("item", io.papermc.paperrs.host.item.HostItemStack.toValue(book)));
    }

    @Override
    public void openBook(final net.kyori.adventure.inventory.Book.Builder book) {
        this.openBook(book.build());
    }

    @Override
    public void openBook(final net.kyori.adventure.inventory.BookLike book) {
        this.openBook(book.asBook());
    }

    @Override
    public void openBook(final net.kyori.adventure.inventory.Book book) {
        final org.bukkit.inventory.ItemStack item = org.bukkit.inventory.ItemStack.of(org.bukkit.Material.WRITTEN_BOOK);
        item.editMeta(org.bukkit.inventory.meta.BookMeta.class, meta -> {
            meta.title(book.title());
            meta.author(book.author());
            meta.pages(book.pages());
        });
        this.openBook(item);
    }

    // --- resource packs -------------------------------------------------------------------------

    private static String hex(final byte @Nullable [] hash) {
        if (hash == null) {
            return "";
        }
        final StringBuilder out = new StringBuilder();
        for (final byte b : hash) {
            out.append(String.format("%02x", b));
        }
        return out.toString();
    }

    private void pushPack(final UUID id, final String url, final String hash, final boolean force, final @Nullable Component prompt) {
        if (url == null) {
            throw new IllegalArgumentException("Resource pack URL cannot be null");
        }
        this.server.send("player.resource_pack_push", this.args("id", id.toString(), "url", url, "hash", hash, "forced", force,
            "prompt", prompt == null ? null : Components.toJson(prompt)));
    }

    private void popPack(final @Nullable UUID id) {
        this.server.send("player.resource_pack_pop", id == null ? this.args() : this.args("id", id.toString()));
    }

    @Override
    @SuppressWarnings("deprecation")
    public void setResourcePack(final String url, final byte @Nullable [] hash, final @Nullable String prompt, final boolean force) {
        this.setResourcePack(UUID.nameUUIDFromBytes(url.getBytes(java.nio.charset.StandardCharsets.UTF_8)), url, hash, prompt == null ? null : Components.legacy(prompt), force);
    }

    @Override
    @SuppressWarnings("deprecation")
    public void setResourcePack(final UUID id, final String url, final byte @Nullable [] hash, final @Nullable String prompt, final boolean force) {
        this.setResourcePack(id, url, hash, prompt == null ? null : Components.legacy(prompt), force);
    }

    /** Replaces every pack the player has, as CraftPlayer does. */
    @Override
    public void setResourcePack(final UUID id, final String url, final byte @Nullable [] hash, final @Nullable Component prompt, final boolean force) {
        this.popPack(null);
        this.pushPack(id, url, hex(hash), force, prompt);
    }

    @Override
    @SuppressWarnings("deprecation")
    public void addResourcePack(final UUID id, final String url, final byte @Nullable [] hash, final @Nullable String prompt, final boolean force) {
        this.pushPack(id, url, hex(hash), force, prompt == null ? null : Components.legacy(prompt));
    }

    @Override
    public void removeResourcePack(final UUID id) {
        this.popPack(id);
    }

    @Override
    public void removeResourcePacks() {
        this.popPack(null);
    }

    @Override
    public org.bukkit.event.player.PlayerResourcePackStatusEvent.@Nullable Status getResourcePackStatus() {
        this.refresh(this.server.call("player.state", this.args()));
        final Object status = this.state.get("resource_pack_status");
        final org.bukkit.event.player.PlayerResourcePackStatusEvent.Status[] all = org.bukkit.event.player.PlayerResourcePackStatusEvent.Status.values();
        return status instanceof Number n && n.intValue() >= 0 && n.intValue() < all.length ? all[n.intValue()] : null;
    }

    @Override
    public void sendResourcePacks(final net.kyori.adventure.resource.ResourcePackInfoLike first, final net.kyori.adventure.resource.ResourcePackInfoLike... others) {
        this.sendResourcePacks(net.kyori.adventure.resource.ResourcePackRequest.addingRequest(first, others));
    }

    @Override
    public void sendResourcePacks(final net.kyori.adventure.resource.ResourcePackRequestLike request) {
        this.sendResourcePacks(request.asResourcePackRequest());
    }

    @Override
    public void sendResourcePacks(final net.kyori.adventure.resource.ResourcePackRequest request) {
        if (request.replace()) {
            this.popPack(null);
        }
        for (final net.kyori.adventure.resource.ResourcePackInfo pack : request.packs()) {
            this.pushPack(pack.id(), pack.uri().toString(), pack.hash(), request.required(), request.prompt());
        }
    }

    @Override
    public void removeResourcePacks(final net.kyori.adventure.resource.ResourcePackRequestLike request) {
        this.removeResourcePacks(request.asResourcePackRequest());
    }

    @Override
    public void removeResourcePacks(final net.kyori.adventure.resource.ResourcePackRequest request) {
        for (final net.kyori.adventure.resource.ResourcePackInfo pack : request.packs()) {
            this.popPack(pack.id());
        }
    }

    @Override
    public void removeResourcePacks(final net.kyori.adventure.resource.ResourcePackInfoLike request, final net.kyori.adventure.resource.ResourcePackInfoLike... others) {
        this.popPack(request.asResourcePackInfo().id());
        for (final net.kyori.adventure.resource.ResourcePackInfoLike other : others) {
            this.popPack(other.asResourcePackInfo().id());
        }
    }

    @Override
    public void removeResourcePacks(final Iterable<UUID> ids) {
        for (final UUID id : ids) {
            this.popPack(id);
        }
    }

    @Override
    public void removeResourcePacks(final UUID id, final UUID... others) {
        this.popPack(id);
        for (final UUID other : others) {
            this.popPack(other);
        }
    }

    @Override
    public void clearResourcePacks() {
        this.popPack(null);
    }

    @Override
    public void showDialog(final net.kyori.adventure.dialog.DialogLike dialog) {
        throw io.papermc.paperrs.host.Unsupported.method("Player#showDialog");
    }

    @Override
    public void closeDialog() {
        throw io.papermc.paperrs.host.Unsupported.method("Player#closeDialog");
    }

    // --- potion effects ----------------------------------------------------------------------

    @Override
    public boolean addPotionEffect(final org.bukkit.potion.PotionEffect effect) {
        return Effects.add(this.server, this.entityId, effect);
    }

    @Override
    public boolean addPotionEffect(final org.bukkit.potion.PotionEffect effect, final boolean force) {
        return Effects.add(this.server, this.entityId, effect);
    }

    @Override
    public boolean addPotionEffects(final java.util.Collection<org.bukkit.potion.PotionEffect> effects) {
        return Effects.addAll(this.server, this.entityId, effects);
    }

    @Override
    public java.util.Collection<org.bukkit.potion.PotionEffect> getActivePotionEffects() {
        return Effects.list(this.server, this.entityId);
    }

    @Override
    public boolean hasPotionEffect(final org.bukkit.potion.PotionEffectType type) {
        return Effects.get(this.server, this.entityId, type) != null;
    }

    @Override
    public org.bukkit.potion.@Nullable PotionEffect getPotionEffect(final org.bukkit.potion.PotionEffectType type) {
        return Effects.get(this.server, this.entityId, type);
    }

    @Override
    public void removePotionEffect(final org.bukkit.potion.PotionEffectType type) {
        Effects.remove(this.server, this.entityId, type);
    }

    @Override
    public boolean clearActivePotionEffects() {
        return Effects.clear(this.server, this.entityId);
    }

    @Override
    public void sendActionBar(final Component message) {
        this.server.send("player.send_message", this.args("message", Components.toJson(message), "overlay", true));
    }

    @Override
    public void sendPlayerListHeaderAndFooter(final Component header, final Component footer) {
        this.listHeader = header;
        this.listFooter = footer;
        this.server.send("player.send_tab_list", this.args("header", Components.toJson(header), "footer", Components.toJson(footer)));
    }

    @Override
    public void showTitle(final Title title) {
        final Title.Times times = title.times();
        final Map<String, Object> args = this.args("title", Components.toJson(title.title()), "subtitle", Components.toJson(title.subtitle()));
        if (times != null) {
            args.put("fade_in", ticks(times.fadeIn()));
            args.put("stay", ticks(times.stay()));
            args.put("fade_out", ticks(times.fadeOut()));
        }
        this.server.send("player.send_title", args);
    }

    @Override
    @SuppressWarnings("unchecked")
    public <T> void sendTitlePart(final TitlePart<T> part, final T value) {
        if (part == TitlePart.TITLE) {
            this.showTitle(Title.title((Component) value, Component.empty()));
        } else if (part == TitlePart.SUBTITLE) {
            this.showTitle(Title.title(Component.empty(), (Component) value));
        }
    }

    @Override
    public void clearTitle() {
        this.server.send("player.clear_title", this.args("reset", false));
    }

    @Override
    public void resetTitle() {
        this.server.send("player.clear_title", this.args("reset", true));
    }

    @Override
    public void sendTitle(final @Nullable String title, final @Nullable String subtitle) {
        this.sendTitle(title, subtitle, 10, 70, 20);
    }

    @Override
    public void sendTitle(final @Nullable String title, final @Nullable String subtitle, final int fadeIn, final int stay, final int fadeOut) {
        this.server.send("player.send_title", this.args(
            "title", Components.toJson(Components.legacy(title == null ? "" : title)),
            "subtitle", Components.toJson(Components.legacy(subtitle == null ? "" : subtitle)),
            "fade_in", fadeIn, "stay", stay, "fade_out", fadeOut));
    }

    private static int ticks(final Duration duration) {
        return (int) (duration.toMillis() / 50);
    }

    @Override
    public void playSound(final net.kyori.adventure.sound.Sound sound) {
        final Location at = this.getLocation();
        this.playSound(sound, at.getX(), at.getY(), at.getZ());
    }

    @Override
    public void playSound(final net.kyori.adventure.sound.Sound sound, final double x, final double y, final double z) {
        this.server.send("player.play_sound", this.args("sound", sound.name().asString(), "category", sound.source().ordinal(),
            "x", x, "y", y, "z", z, "volume", (double) sound.volume(), "pitch", (double) sound.pitch()));
    }

    @Override
    public void sendPluginMessage(final Plugin source, final String channel, final byte[] message) {
        StandardMessenger.validatePluginMessage(this.server.getMessenger(), source, channel, message);
        this.server.send("player.send_plugin_message", this.args("channel", channel, "data", message));
    }

    @Override
    public Set<String> getListeningPluginChannels() {
        return Set.of();
    }

    @Override
    public boolean performCommand(final String command) {
        return this.server.dispatchCommand(this, command);
    }

    @Override
    public void chat(final String message) {
        this.server.call("player.chat", this.args("message", message));
    }

    @Override
    public void updateCommands() {
        this.server.send("player.update_commands", this.args());
    }

    // --- position ---------------------------------------------------------------------------

    @Override
    public World getWorld() {
        final World world = this.server.getWorld(Values.str(this.state, "world", "world"));
        return world != null ? world : this.server.getWorlds().getFirst();
    }

    @Override
    public Location getLocation() {
        return new Location(this.getWorld(), Values.f64(this.state, "x", 0), Values.f64(this.state, "y", 0), Values.f64(this.state, "z", 0),
            (float) Values.f64(this.state, "yaw", 0), (float) Values.f64(this.state, "pitch", 0));
    }

    @Override
    public @Nullable Location getLocation(final @Nullable Location loc) {
        if (loc != null) {
            final Location here = this.getLocation();
            loc.setWorld(here.getWorld());
            loc.set(here.getX(), here.getY(), here.getZ());
            loc.setYaw(here.getYaw());
            loc.setPitch(here.getPitch());
        }
        return loc;
    }

    @Override
    public double getX() {
        return Values.f64(this.state, "x", 0);
    }

    @Override
    public double getY() {
        return Values.f64(this.state, "y", 0);
    }

    @Override
    public double getZ() {
        return Values.f64(this.state, "z", 0);
    }

    @Override
    public float getYaw() {
        return (float) Values.f64(this.state, "yaw", 0);
    }

    @Override
    public float getPitch() {
        return (float) Values.f64(this.state, "pitch", 0);
    }

    @Override
    public BlockFace getFacing() {
        final float yaw = Math.floorMod((int) Math.floor(this.getYaw() + 45), 360);
        if (yaw < 90) {
            return BlockFace.SOUTH;
        }
        if (yaw < 180) {
            return BlockFace.WEST;
        }
        return yaw < 270 ? BlockFace.NORTH : BlockFace.EAST;
    }

    @Override
    public double getEyeHeight() {
        return this.isSneaking() ? 1.27 : 1.62;
    }

    @Override
    public double getEyeHeight(final boolean ignorePose) {
        return ignorePose ? 1.62 : this.getEyeHeight();
    }

    @Override
    public Location getEyeLocation() {
        return this.getLocation().add(0, this.getEyeHeight(), 0);
    }

    @Override
    public boolean isOnGround() {
        return Values.bool(this.state, "on_ground", false);
    }

    @Override
    public Vector getVelocity() {
        return new Vector(0, 0, 0);
    }

    @Override
    public boolean teleport(final Location location) {
        return this.teleport(location, PlayerTeleportEvent.TeleportCause.PLUGIN);
    }

    @Override
    public boolean teleport(final Location location, final PlayerTeleportEvent.TeleportCause cause) {
        return this.teleport(location, cause, new TeleportFlag[0]);
    }

    @Override
    public boolean teleport(final Location location, final PlayerTeleportEvent.TeleportCause cause, final TeleportFlag... flags) {
        final Location from = this.getLocation();
        final Location target = location.clone();
        if (target.getWorld() == null) {
            target.setWorld(from.getWorld());
        }
        final PlayerTeleportEvent event = new PlayerTeleportEvent(this, from, target, cause, Set.of(flags).stream()
            .filter(f -> f instanceof TeleportFlag.Relative).map(f -> (TeleportFlag.Relative) f).collect(java.util.stream.Collectors.toUnmodifiableSet()));
        if (!event.callEvent()) {
            return false;
        }
        final Location to = event.getTo();
        try {
            this.refresh(this.server.call("player.teleport", this.args("world", to.getWorld() == null ? from.getWorld().getName() : to.getWorld().getName(),
                "x", to.getX(), "y", to.getY(), "z", to.getZ(), "yaw", (double) to.getYaw(), "pitch", (double) to.getPitch())));
            return true;
        } catch (RuntimeException e) {
            this.server.getLogger().warning("Could not teleport " + this.name + ": " + e.getMessage());
            return false;
        }
    }

    @Override
    public boolean teleport(final Entity destination) {
        return this.teleport(destination.getLocation());
    }

    @Override
    public boolean teleport(final Entity destination, final PlayerTeleportEvent.TeleportCause cause) {
        return this.teleport(destination.getLocation(), cause);
    }

    @Override
    public CompletableFuture<Boolean> teleportAsync(final Location location, final PlayerTeleportEvent.TeleportCause cause, final TeleportFlag... flags) {
        final CompletableFuture<Boolean> result = new CompletableFuture<>();
        if (this.server.isPrimaryThread()) {
            result.complete(this.teleport(location, cause, flags));
        } else {
            this.server.runOnMain(() -> result.complete(this.teleport(location, cause, flags)));
        }
        return result;
    }

    // --- game state -------------------------------------------------------------------------

    @Override
    public GameMode getGameMode() {
        return GameMode.valueOf(Values.str(this.state, "game_mode", "SURVIVAL").toUpperCase(Locale.ROOT));
    }

    @Override
    public void setGameMode(final GameMode mode) {
        this.refresh(this.server.call("player.set_game_mode", this.args("mode", mode.name().toLowerCase(Locale.ROOT))));
    }

    @Override
    public double getHealth() {
        return Values.f64(this.state, "health", 20);
    }

    @Override
    public void setHealth(final double health) {
        if (health < 0 || health > this.getMaxHealth()) {
            throw new IllegalArgumentException("Health value (" + health + ") must be between 0 and " + this.getMaxHealth());
        }
        this.server.call("player.set_health", this.args("health", health));
        this.state.put("health", health);
    }

    @Override
    public double getMaxHealth() {
        final org.bukkit.attribute.AttributeInstance max = this.getAttribute(org.bukkit.attribute.Attribute.MAX_HEALTH);
        return max == null ? 20 : max.getValue();
    }

    @Override
    @SuppressWarnings("deprecation")
    public void setMaxHealth(final double health) {
        final org.bukkit.attribute.AttributeInstance max = this.getAttribute(org.bukkit.attribute.Attribute.MAX_HEALTH);
        if (max != null) {
            max.setBaseValue(health);
        }
    }

    @Override
    @SuppressWarnings("deprecation")
    public void resetMaxHealth() {
        this.setMaxHealth(20);
    }

    @Override
    public double getHealthScale() {
        return 20;
    }

    @Override
    public boolean isDead() {
        return this.getHealth() <= 0;
    }

    @Override
    public int getFoodLevel() {
        return Values.i32(this.state, "food", 20);
    }

    @Override
    public void setFoodLevel(final int value) {
        this.server.call("player.set_health", this.args("food", value));
        this.state.put("food", (long) value);
    }

    @Override
    public float getSaturation() {
        return (float) Values.f64(this.state, "saturation", 5);
    }

    @Override
    public void setSaturation(final float value) {
        this.server.call("player.set_health", this.args("saturation", (double) value));
        this.state.put("saturation", (double) value);
    }

    @Override
    public boolean isSneaking() {
        return Values.bool(this.state, "sneaking", false);
    }

    @Override
    public boolean isSprinting() {
        return Values.bool(this.state, "sprinting", false);
    }

    @Override
    public boolean isFlying() {
        return Values.bool(this.state, "flying", false);
    }

    @Override
    public void setFlying(final boolean value) {
        if (value && !this.getAllowFlight()) {
            throw new IllegalArgumentException("Player is not allowed to fly (check #getAllowFlight())");
        }
        this.server.call("player.set_flight", this.args("flying", value));
        this.state.put("flying", value);
    }

    @Override
    public boolean getAllowFlight() {
        return Values.bool(this.state, "allow_flight", false);
    }

    @Override
    public void setAllowFlight(final boolean flight) {
        this.server.call("player.set_flight", this.args("allow_flight", flight));
        this.state.put("allow_flight", flight);
        if (!flight) {
            this.state.put("flying", false);
        }
    }

    @Override
    public boolean isInvisible() {
        return false;
    }

    @Override
    public boolean canSee(final Player player) {
        return true;
    }

    @Override
    public boolean canSee(final Entity entity) {
        return true;
    }

    // --- permissions ------------------------------------------------------------------------

    @Override
    public boolean isOp() {
        return Values.i64(this.state, "op_level", 0) > 0;
    }

    @Override
    public void setOp(final boolean value) {
        if (value == this.isOp()) {
            return;
        }
        this.server.call("player.set_op", this.args("op", value));
        this.state.put("op_level", value ? 4L : 0L);
        this.perm.recalculatePermissions();
    }

    @Override
    public boolean isPermissionSet(final String name) {
        return this.perm.isPermissionSet(name);
    }

    @Override
    public boolean isPermissionSet(final Permission perm) {
        return this.perm.isPermissionSet(perm);
    }

    @Override
    public boolean hasPermission(final String name) {
        return this.perm.hasPermission(name);
    }

    @Override
    public boolean hasPermission(final Permission perm) {
        return this.perm.hasPermission(perm);
    }

    @Override
    public PermissionAttachment addAttachment(final Plugin plugin, final String name, final boolean value) {
        return this.perm.addAttachment(plugin, name, value);
    }

    @Override
    public PermissionAttachment addAttachment(final Plugin plugin) {
        return this.perm.addAttachment(plugin);
    }

    @Override
    public @Nullable PermissionAttachment addAttachment(final Plugin plugin, final String name, final boolean value, final int ticks) {
        return this.perm.addAttachment(plugin, name, value, ticks);
    }

    @Override
    public @Nullable PermissionAttachment addAttachment(final Plugin plugin, final int ticks) {
        return this.perm.addAttachment(plugin, ticks);
    }

    @Override
    public void removeAttachment(final PermissionAttachment attachment) {
        this.perm.removeAttachment(attachment);
    }

    @Override
    public void recalculatePermissions() {
        this.perm.recalculatePermissions();
    }

    @Override
    public Set<PermissionAttachmentInfo> getEffectivePermissions() {
        return this.perm.getEffectivePermissions();
    }

    // --- inventories --------------------------------------------------------------------------

    @Override
    public org.bukkit.inventory.PlayerInventory getInventory() {
        if (this.inventory == null) {
            this.inventory = io.papermc.paperrs.host.inventory.Inventories.player(this.server, this);
        }
        return this.inventory;
    }

    @Override
    public org.bukkit.inventory.Inventory getEnderChest() {
        if (this.enderChest == null) {
            this.enderChest = io.papermc.paperrs.host.inventory.Inventories.enderChest(this.server, this);
        }
        return this.enderChest;
    }

    @Override
    public org.bukkit.inventory.EntityEquipment getEquipment() {
        return HostEquipment.of(this);
    }

    /** The selected hotbar slot (0-8). */
    public int getHeldSlot() {
        return Values.i32(this.state, "held_slot", 0);
    }

    public void setHeldSlot(final int slot) {
        this.server.call("player.set_held_slot", this.args("slot", slot));
        this.state.put("held_slot", (long) slot);
    }

    @Override
    @SuppressWarnings("deprecation")
    public org.bukkit.inventory.ItemStack getItemInHand() {
        return this.getInventory().getItemInMainHand();
    }

    @Override
    @SuppressWarnings("deprecation")
    public void setItemInHand(final org.bukkit.inventory.@Nullable ItemStack item) {
        this.getInventory().setItemInMainHand(item);
    }

    @Override
    public org.bukkit.inventory.ItemStack getItemOnCursor() {
        return io.papermc.paperrs.host.item.HostItemStack.fromValue(this.server.call("player.cursor", this.args()));
    }

    @Override
    public void setItemOnCursor(final org.bukkit.inventory.@Nullable ItemStack item) {
        this.server.call("player.set_cursor", this.args("item", io.papermc.paperrs.host.item.HostItemStack.toValue(item)));
    }

    @Override
    public org.bukkit.inventory.InventoryView getOpenInventory() {
        return io.papermc.paperrs.host.inventory.Inventories.view(this.server, this, Values.map(this.state.get("open_inventory")));
    }

    @Override
    public org.bukkit.inventory.@Nullable InventoryView openInventory(final org.bukkit.inventory.Inventory inventory) {
        final io.papermc.paperrs.host.inventory.HostInventory impl = io.papermc.paperrs.host.HostProxies.implOf(inventory, io.papermc.paperrs.host.inventory.HostInventory.class);
        if (impl == null) {
            throw new IllegalArgumentException("not an inventory made by Paper-RS: " + inventory);
        }
        final Map<String, Object> result = Values.map(this.server.call("player.open_inventory",
            this.args("inventory", impl.ref(), "title", Components.toJson(impl.title()))));
        this.refresh(this.server.call("player.state", this.args()));
        if (!Values.bool(result, "opened", false)) {
            return null;
        }
        this.openTop = inventory;
        return this.getOpenInventory();
    }

    @Override
    public void openInventory(final org.bukkit.inventory.InventoryView view) {
        this.openInventory(view.getTopInventory());
    }

    /**
     * {@code HumanEntity#openMerchant}: trading with a villager, wandering trader or a merchant
     * a plugin made. Without {@code force}, a merchant already trading with someone stays
     * with them ({@code null} is returned); with it, their window closes.
     */
    @Override
    public org.bukkit.inventory.@Nullable InventoryView openMerchant(final org.bukkit.inventory.Merchant merchant, final boolean force) {
        if (merchant == null) {
            throw new IllegalArgumentException("Merchant cannot be null");
        }
        final Map<String, Object> ref;
        if (merchant instanceof HostMerchant custom) {
            ref = Values.of("merchant", custom.id());
        } else if (io.papermc.paperrs.host.HostProxies.implOf(merchant, HostEntity.class) instanceof HostEntity entity) {
            ref = Values.of("entity", entity.id());
        } else {
            throw new IllegalArgumentException("Can't open merchant " + merchant);
        }
        if (!force && merchant.isTrading()) {
            return null;
        }
        final Map<String, Object> args = this.args("force", force);
        args.putAll(ref);
        final Map<String, Object> result = Values.map(this.server.call("merchant.open", args));
        this.refresh(this.server.call("player.state", this.args()));
        return Values.bool(result, "opened", false) ? this.getOpenInventory() : null;
    }

    @Override
    public void closeInventory() {
        this.closeInventory(org.bukkit.event.inventory.InventoryCloseEvent.Reason.PLUGIN);
    }

    @Override
    public void closeInventory(final org.bukkit.event.inventory.InventoryCloseEvent.Reason reason) {
        this.server.call("player.close_inventory", this.args("reason", reason.name()));
        this.openTop = null;
        this.refresh(this.server.call("player.state", this.args()));
    }

    /** The window closed (InventoryCloseEvent): the opened inventory need not be kept. */
    public void windowClosed() {
        this.openTop = null;
    }

    // --- scoreboard ---------------------------------------------------------------------------

    @Override
    public org.bukkit.scoreboard.Scoreboard getScoreboard() {
        return io.papermc.paperrs.host.scoreboard.HostScoreboards.board(this.server, Values.i64(this.state, "scoreboard", 0));
    }

    @Override
    public void setScoreboard(final org.bukkit.scoreboard.Scoreboard scoreboard) {
        final long id = io.papermc.paperrs.host.scoreboard.HostScoreboards.idOf(scoreboard);
        this.server.call("player.set_scoreboard", this.args("board", id));
        this.state.put("scoreboard", id);
    }

    // --- offline player data ------------------------------------------------------------------

    private Map<String, Object> offlineInfo() {
        return Values.map(this.server.call("offline.get", Values.of("uuid", this.uuid.toString())));
    }

    @Override
    public boolean hasPlayedBefore() {
        return Values.bool(this.offlineInfo(), "played_before", false);
    }

    @Override
    public long getFirstPlayed() {
        return Values.i64(this.offlineInfo(), "first_played", 0);
    }

    @Override
    @SuppressWarnings("deprecation")
    public long getLastPlayed() {
        return System.currentTimeMillis();
    }

    @Override
    public long getLastLogin() {
        return Values.i64(this.offlineInfo(), "last_login", 0);
    }

    @Override
    public long getLastSeen() {
        return System.currentTimeMillis();
    }

    @Override
    public boolean isBanned() {
        return Values.bool(this.offlineInfo(), "banned", false);
    }

    @Override
    public boolean isWhitelisted() {
        return Values.bool(this.offlineInfo(), "whitelisted", false);
    }

    @Override
    public void setWhitelisted(final boolean value) {
        this.server.call("offline.set_whitelisted", Values.of("uuid", this.uuid.toString(), "name", this.name, "value", value));
    }

    @Override
    public org.bukkit.entity.Player getPlayer() {
        return this;
    }

    // --- living entity --------------------------------------------------------------------------

    @Override
    public void damage(final double amount) {
        this.server.call("entity.damage", Values.of("id", this.entityId, "amount", amount));
    }

    @Override
    public void damage(final double amount, final @Nullable Entity source) {
        this.server.call("entity.damage", Values.of("id", this.entityId, "amount", amount, "source", Entities.ref(source)));
    }

    @Override
    public int getFireTicks() {
        return Values.i32(this.state, "fire_ticks", 0);
    }

    @Override
    public int getRemainingAir() {
        return Values.i32(this.state, "air", 300);
    }

    @Override
    public int getMaximumAir() {
        return 300;
    }

    @Override
    public int getLevel() {
        return Values.i32(this.state, "xp_level", 0);
    }

    @Override
    public float getExp() {
        return (float) Values.f64(this.state, "xp_progress", 0);
    }

    @Override
    public int getTotalExperience() {
        return Values.i32(this.state, "xp_total", 0);
    }

    @Override
    public void giveExp(final int amount) {
        this.server.call("player.give_exp", this.args("amount", amount));
    }

    @Override
    public List<Entity> getNearbyEntities(final double x, final double y, final double z) {
        final List<Entity> out = new java.util.ArrayList<>(this.getWorld().getNearbyEntities(this.getLocation(), x, y, z));
        out.remove(this);
        return out;
    }

    @Override
    public String getScoreboardEntryName() {
        return this.name;
    }

    @Override
    public double getWidth() {
        return 0.6;
    }

    @Override
    public double getHeight() {
        return this.isSneaking() ? 1.5 : 1.8;
    }

    @Override
    public org.bukkit.util.BoundingBox getBoundingBox() {
        final double x = this.getX(), y = this.getY(), z = this.getZ();
        return new org.bukkit.util.BoundingBox(x - 0.3, y, z - 0.3, x + 0.3, y + this.getHeight(), z + 0.3);
    }

    // --- metadata ---------------------------------------------------------------------------

    @Override
    public void setMetadata(final String metadataKey, final MetadataValue newMetadataValue) {
        this.server.playerMetadata().setMetadata(this, metadataKey, newMetadataValue);
    }

    @Override
    public List<MetadataValue> getMetadata(final String metadataKey) {
        return this.server.playerMetadata().getMetadata(this, metadataKey);
    }

    @Override
    public boolean hasMetadata(final String metadataKey) {
        return this.server.playerMetadata().hasMetadata(this, metadataKey);
    }

    @Override
    public void removeMetadata(final String metadataKey, final Plugin owningPlugin) {
        this.server.playerMetadata().removeMetadata(this, metadataKey, owningPlugin);
    }

    @Override
    public Player.Spigot spigot() {
        return new Player.Spigot() {
        };
    }

    @Override
    public boolean equals(final Object o) {
        return o instanceof HostPlayer p && p.uuid.equals(this.uuid);
    }

    @Override
    public int hashCode() {
        return this.uuid.hashCode();
    }

    @Override
    public String toString() {
        return "CraftPlayer{name=" + this.name + "}";
    }

    // --- statistics -------------------------------------------------------------------------

    @Override
    public int getStatistic(final org.bukkit.Statistic statistic) {
        return Statistics.get(this.server, this.uuid, statistic);
    }

    @Override
    public void setStatistic(final org.bukkit.Statistic statistic, final int newValue) {
        Statistics.set(this.server, this.uuid, statistic, newValue);
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic) {
        Statistics.add(this.server, this.uuid, statistic, Statistics.amount(1));
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, Statistics.amount(amount));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic) {
        Statistics.add(this.server, this.uuid, statistic, -Statistics.amount(1));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, -Statistics.amount(amount));
    }

    @Override
    public int getStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material) {
        return Statistics.get(this.server, this.uuid, statistic, material);
    }

    @Override
    public void setStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material, final int newValue) {
        Statistics.set(this.server, this.uuid, statistic, material, newValue);
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material) {
        Statistics.add(this.server, this.uuid, statistic, material, Statistics.amount(1));
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, material, Statistics.amount(amount));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material) {
        Statistics.add(this.server, this.uuid, statistic, material, -Statistics.amount(1));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.Material material, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, material, -Statistics.amount(amount));
    }

    @Override
    public int getStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType) {
        return Statistics.get(this.server, this.uuid, statistic, entityType);
    }

    @Override
    public void setStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType, final int newValue) {
        Statistics.set(this.server, this.uuid, statistic, entityType, newValue);
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType) {
        Statistics.add(this.server, this.uuid, statistic, entityType, Statistics.amount(1));
    }

    @Override
    public void incrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, entityType, Statistics.amount(amount));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType) {
        Statistics.add(this.server, this.uuid, statistic, entityType, -Statistics.amount(1));
    }

    @Override
    public void decrementStatistic(final org.bukkit.Statistic statistic, final org.bukkit.entity.EntityType entityType, final int amount) {
        Statistics.add(this.server, this.uuid, statistic, entityType, -Statistics.amount(amount));
    }
}
