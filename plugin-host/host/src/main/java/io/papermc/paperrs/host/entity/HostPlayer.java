package io.papermc.paperrs.host.entity;

import com.destroystokyo.paper.profile.PlayerProfile;
import com.destroystokyo.paper.profile.ProfileProperty;
import io.papermc.paper.entity.TeleportFlag;
import io.papermc.paperrs.host.Components;
import io.papermc.paperrs.host.HostPlayerProfile;
import io.papermc.paperrs.host.bridge.Values;
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
public final class HostPlayer extends org.bukkit.craftbukkit.entity.CraftHumanEntity {
    private final CraftServer server;
    private final UUID uuid;
    private final String name;
    private final int entityId;
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

    // --- Bukkit's player and entity state (see plugins/player.rs) ------------------------------

    /** {@code player.set}: changes plain player state; the server answers with the new state. */
    private void set(final Object... kv) {
        this.refresh(this.server.call("player.set", this.args(kv)));
    }

    private static void checkSpeed(final float value) {
        com.google.common.base.Preconditions.checkArgument(value <= 1f && value >= -1f, "Speed value (%s) need to be between -1f and 1f", value);
    }

    @Override
    public float getWalkSpeed() {
        return (float) Values.f64(this.state, "walk_speed", 0.2);
    }

    @Override
    public void setWalkSpeed(final float value) {
        checkSpeed(value);
        this.set("walk_speed", (double) value);
    }

    @Override
    public float getFlySpeed() {
        return (float) Values.f64(this.state, "fly_speed", 0.1);
    }

    @Override
    public void setFlySpeed(final float value) {
        checkSpeed(value);
        this.set("fly_speed", (double) value);
    }

    @Override
    public boolean isSleepingIgnored() {
        return Values.bool(this.state, "sleeping_ignored", false);
    }

    @Override
    public void setSleepingIgnored(final boolean isSleeping) {
        this.set("sleeping_ignored", isSleeping);
    }

    @Override
    public boolean isSleeping() {
        return this.state.get("sleeping") != null;
    }

    @Override
    public boolean isDeeplySleeping() {
        return this.isSleeping() && this.getSleepTicks() >= 100;
    }

    @Override
    public int getSleepTicks() {
        return Values.i32(this.state, "sleep_ticks", 0);
    }

    @Override
    public Location getBedLocation() {
        com.google.common.base.Preconditions.checkState(this.isSleeping(), "Not sleeping");
        final List<Object> bed = Values.list(this.state.get("sleeping"));
        return new Location(this.getWorld(), ((Number) bed.get(0)).intValue(), ((Number) bed.get(1)).intValue(), ((Number) bed.get(2)).intValue());
    }

    // time and weather this player sees

    @Override
    public void setPlayerTime(final long time, final boolean relative) {
        this.set("time", time, "relative", relative);
    }

    @Override
    public long getPlayerTime() {
        return Values.i64(this.state, "player_time", 0);
    }

    @Override
    public long getPlayerTimeOffset() {
        return Values.i64(this.state, "player_time_offset", 0);
    }

    @Override
    public boolean isPlayerTimeRelative() {
        return Values.bool(this.state, "player_time_relative", true);
    }

    @Override
    public void resetPlayerTime() {
        this.set("reset_time", true);
    }

    @Override
    public void setPlayerWeather(final org.bukkit.WeatherType type) {
        this.set("weather", type.name());
    }

    @Override
    public org.bukkit.@Nullable WeatherType getPlayerWeather() {
        final Object weather = this.state.get("player_weather");
        return weather == null ? null : org.bukkit.WeatherType.valueOf(weather.toString());
    }

    @Override
    public void resetPlayerWeather() {
        this.set("reset_weather", true);
    }

    // entity flags

    @Override
    public boolean isInvulnerable() {
        return Values.bool(this.state, "invulnerable", false);
    }

    @Override
    public void setInvulnerable(final boolean flag) {
        this.set("invulnerable", flag);
    }

    @Override
    public boolean isGlowing() {
        return Values.bool(this.state, "glowing", false);
    }

    @Override
    public void setGlowing(final boolean flag) {
        this.set("glowing", flag);
    }

    @Override
    public boolean isSilent() {
        return Values.bool(this.state, "silent", false);
    }

    @Override
    public void setSilent(final boolean flag) {
        this.set("silent", flag);
    }

    @Override
    public boolean hasGravity() {
        return !Values.bool(this.state, "no_gravity", false);
    }

    @Override
    public void setGravity(final boolean gravity) {
        this.set("gravity", gravity);
    }

    @Override
    public boolean isInvisible() {
        return Values.bool(this.state, "invisible", false);
    }

    @Override
    public void setInvisible(final boolean invisible) {
        this.set("invisible", invisible);
    }

    @Override
    public @Nullable Component customName() {
        return Components.fromJson(this.state.get("custom_name"));
    }

    @Override
    public void customName(final @Nullable Component customName) {
        if (customName == null) {
            this.set("clear_custom_name", true);
        } else {
            this.set("custom_name", Components.toJson(customName));
        }
    }

    @Override
    public @Nullable String getCustomName() {
        final Component name = this.customName();
        return name == null ? null : Components.toLegacy(name);
    }

    @Override
    public void setCustomName(final @Nullable String name) {
        this.customName(name == null ? null : Components.legacy(name));
    }

    @Override
    public boolean isCustomNameVisible() {
        return Values.bool(this.state, "custom_name_visible", false);
    }

    @Override
    public void setCustomNameVisible(final boolean flag) {
        this.set("custom_name_visible", flag);
    }

    @Override
    public Set<String> getScoreboardTags() {
        final Set<String> tags = new java.util.LinkedHashSet<>();
        for (final Object tag : Values.list(this.state.get("tags"))) {
            tags.add(String.valueOf(tag));
        }
        return tags;
    }

    @Override
    public boolean addScoreboardTag(final String tag) {
        final Set<String> tags = this.getScoreboardTags();
        if (tags.size() >= 1024 || tags.contains(tag)) {
            return false;
        }
        this.set("add_tag", tag);
        return true;
    }

    @Override
    public boolean removeScoreboardTag(final String tag) {
        if (!this.getScoreboardTags().contains(tag)) {
            return false;
        }
        this.set("remove_tag", tag);
        return true;
    }

    @Override
    public int getTicksLived() {
        return Math.max(1, Values.i32(this.state, "ticks_lived", 1));
    }

    @Override
    public void setTicksLived(final int value) {
        com.google.common.base.Preconditions.checkArgument(value > 0, "Age value (%s) must be greater than 0", value);
        this.set("ticks_lived", value);
    }

    @Override
    public int getPortalCooldown() {
        return Values.i32(this.state, "portal_cooldown", 0);
    }

    @Override
    public void setPortalCooldown(final int cooldown) {
        com.google.common.base.Preconditions.checkArgument(cooldown >= 0, "Portal cooldown (%s) must be beq 0", cooldown);
        this.set("portal_cooldown", cooldown);
    }

    @Override
    public void setSneaking(final boolean sneak) {
        this.set("sneaking", sneak);
    }

    @Override
    public void setSprinting(final boolean sprinting) {
        this.set("sprinting", sprinting);
    }

    @Override
    public void setVelocity(final Vector velocity) {
        velocity.checkFinite();
        this.set("vx", velocity.getX(), "vy", velocity.getY(), "vz", velocity.getZ());
    }

    @Override
    public void knockback(final double strength, final double directionX, final double directionZ) {
        com.google.common.base.Preconditions.checkArgument(strength > 0, "Knockback strength must be > 0");
        // LivingEntity.knockback: half the current motion, pushed away from the direction.
        final Vector now = this.getVelocity();
        final Vector push = new Vector(directionX, 0, directionZ).normalize().multiply(strength);
        this.setVelocity(new Vector(now.getX() / 2 - push.getX(), this.isOnGround() ? Math.min(0.4, now.getY() / 2 + strength) : now.getY(), now.getZ() / 2 - push.getZ()));
    }

    @Override
    public org.bukkit.entity.Pose getPose() {
        if (this.isDead()) {
            return org.bukkit.entity.Pose.DYING;
        }
        if (this.isSleeping()) {
            return org.bukkit.entity.Pose.SLEEPING;
        }
        return this.isSneaking() ? org.bukkit.entity.Pose.SNEAKING : org.bukkit.entity.Pose.STANDING;
    }

    // air, fire, freezing, damage ticks

    @Override
    public int getMaximumAir() {
        return Values.i32(this.state, "max_air", 300);
    }

    @Override
    public void setMaximumAir(final int ticks) {
        this.set("max_air", ticks);
    }

    @Override
    public void setRemainingAir(final int ticks) {
        this.set("air", ticks);
    }

    @Override
    public void setFireTicks(final int ticks) {
        this.set("fire_ticks", ticks);
    }

    @Override
    public int getMaxFireTicks() {
        // Player.getFireImmuneTicks
        return 20;
    }

    @Override
    public int getFreezeTicks() {
        return Values.i32(this.state, "freeze_ticks", 0);
    }

    @Override
    public void setFreezeTicks(final int ticks) {
        com.google.common.base.Preconditions.checkArgument(0 <= ticks, "Ticks (%s) cannot be less than 0", ticks);
        this.set("freeze_ticks", ticks);
    }

    @Override
    public int getMaxFreezeTicks() {
        return 140;
    }

    @Override
    public boolean isFrozen() {
        return this.getFreezeTicks() >= this.getMaxFreezeTicks();
    }

    private boolean freezeLocked;

    @Override
    public void lockFreezeTicks(final boolean locked) {
        this.freezeLocked = locked;
    }

    @Override
    public boolean isFreezeTickingLocked() {
        return this.freezeLocked;
    }

    @Override
    public float getFallDistance() {
        return (float) Values.f64(this.state, "fall_distance", 0);
    }

    @Override
    public void setFallDistance(final float distance) {
        this.set("fall_distance", (double) distance);
    }

    @Override
    public int getNoDamageTicks() {
        return Values.i32(this.state, "no_damage_ticks", 0);
    }

    @Override
    public void setNoDamageTicks(final int ticks) {
        this.set("no_damage_ticks", ticks);
    }

    @Override
    public int getMaximumNoDamageTicks() {
        return Values.i32(this.state, "max_no_damage_ticks", 20);
    }

    @Override
    public void setMaximumNoDamageTicks(final int ticks) {
        this.set("max_no_damage_ticks", ticks);
    }

    @Override
    public double getLastDamage() {
        return Values.f64(this.state, "last_damage", 0);
    }

    @Override
    public void setLastDamage(final double damage) {
        this.set("last_damage", damage);
    }

    @Override
    public int getArrowsInBody() {
        return Values.i32(this.state, "arrows_in_body", 0);
    }

    @Override
    public void setArrowsInBody(final int count, final boolean fireEvent) {
        com.google.common.base.Preconditions.checkArgument(count >= 0, "New arrow amount must be >= 0");
        this.set("arrows_in_body", count);
    }

    @Override
    public int getBeeStingersInBody() {
        return Values.i32(this.state, "stingers_in_body", 0);
    }

    @Override
    public void setBeeStingersInBody(final int count) {
        com.google.common.base.Preconditions.checkArgument(count >= 0, "New bee stinger amount must be >= 0");
        this.set("stingers_in_body", count);
    }

    @Override
    public double getAbsorptionAmount() {
        return Values.f64(this.state, "absorption", 0);
    }

    @Override
    public void setAbsorptionAmount(final double amount) {
        com.google.common.base.Preconditions.checkArgument(amount >= 0 && Double.isFinite(amount), "amount < 0 or non-finite");
        this.set("absorption", amount);
    }

    @Override
    public float getExhaustion() {
        return (float) Values.f64(this.state, "exhaustion", 0);
    }

    @Override
    public void setExhaustion(final float value) {
        this.set("exhaustion", (double) value);
    }

    @Override
    public void heal(final double amount, final org.bukkit.event.entity.EntityRegainHealthEvent.RegainReason reason) {
        com.google.common.base.Preconditions.checkArgument(amount > 0, "amount must be greater than 0");
        this.setHealth(Math.min(this.getMaxHealth(), this.getHealth() + amount));
    }

    @Override
    public @Nullable Player getKiller() {
        final Object killer = this.state.get("killer");
        return killer == null ? null : this.server.getPlayer(UUID.fromString(killer.toString()));
    }

    private org.bukkit.event.entity.@Nullable EntityDamageEvent lastDamageCause;

    @Override
    public org.bukkit.event.entity.@Nullable EntityDamageEvent getLastDamageCause() {
        return this.lastDamageCause;
    }

    @Override
    public void setLastDamageCause(final org.bukkit.event.entity.@Nullable EntityDamageEvent event) {
        this.lastDamageCause = event;
    }

    // health scale

    @Override
    public void setHealthScale(final double scale) {
        com.google.common.base.Preconditions.checkArgument(scale > 0, "Must be greater than 0");
        this.set("health_scale", scale, "health_scaled", true);
    }

    @Override
    public double getHealthScale() {
        return Values.f64(this.state, "health_scale", 20.0);
    }

    @Override
    public void setHealthScaled(final boolean scale) {
        this.set("health_scaled", scale);
    }

    @Override
    public boolean isHealthScaled() {
        return this.state.get("health_scale") != null;
    }

    // experience

    @Override
    public void setExp(final float exp) {
        com.google.common.base.Preconditions.checkArgument(exp >= 0.0 && exp <= 1.0, "Experience progress must be between 0.0 and 1.0 (%s)", exp);
        this.set("xp_progress", (double) exp);
    }

    @Override
    public void setLevel(final int level) {
        com.google.common.base.Preconditions.checkArgument(level >= 0, "Experience level must not be negative (%s)", level);
        this.set("xp_level", level);
    }

    @Override
    public void setTotalExperience(final int exp) {
        com.google.common.base.Preconditions.checkArgument(exp >= 0, "Total experience points must not be negative (%s)", exp);
        this.set("xp_total", exp);
    }

    @Override
    public void giveExpLevels(final int amount) {
        // Player.giveExperienceLevels: below zero resets the experience.
        final int level = this.getLevel() + amount;
        if (level < 0) {
            this.set("xp_level", 0, "xp_progress", 0.0, "xp_total", 0);
        } else {
            this.set("xp_level", level);
        }
    }

    /** {@code Player.getXpNeededForNextLevel}. */
    private static int xpNeeded(final int level) {
        return level >= 30 ? 112 + (level - 30) * 9 : level >= 15 ? 37 + (level - 15) * 5 : 7 + level * 2;
    }

    @Override
    public int getExpToLevel() {
        return xpNeeded(this.getLevel());
    }

    @Override
    public int getExperiencePointsNeededForNextLevel() {
        return xpNeeded(this.getLevel());
    }

    @Override
    public int calculateTotalExperiencePoints() {
        int points = 0;
        for (int level = 0; level < this.getLevel(); level++) {
            points += xpNeeded(level);
        }
        return points + Math.round(xpNeeded(this.getLevel()) * this.getExp());
    }

    @Override
    public void setExperienceLevelAndProgress(final int totalExperience) {
        com.google.common.base.Preconditions.checkArgument(totalExperience >= 0, "totalExperience must be a positive integer");
        int level = 0;
        int left = totalExperience;
        while (left >= xpNeeded(level)) {
            left -= xpNeeded(level);
            level++;
        }
        this.set("xp_level", level, "xp_progress", (double) left / xpNeeded(level));
    }

    @Override
    public void sendExperienceChange(final float progress) {
        this.sendExperienceChange(progress, this.getLevel());
    }

    @Override
    public void sendExperienceChange(final float progress, final int level) {
        com.google.common.base.Preconditions.checkArgument(progress >= 0.0 && progress <= 1.0, "Experience progress must be between 0.0 and 1.0 (%s)", progress);
        com.google.common.base.Preconditions.checkArgument(level >= 0, "Experience level must not be negative (%s)", level);
        this.server.send("player.send_experience", this.args("progress", (double) progress, "level", level));
    }

    @Override
    public void sendHealthUpdate() {
        this.sendHealthUpdate(this.getHealth(), this.getFoodLevel(), this.getSaturation());
    }

    @Override
    public void sendHealthUpdate(final double health, final int foodLevel, final float saturation) {
        this.server.send("player.send_health", this.args("health", health, "food", foodLevel, "saturation", (double) saturation));
    }

    // compass, respawn point

    @Override
    public Location getCompassTarget() {
        final List<Object> target = Values.list(this.state.get("compass_target"));
        if (target.size() < 4) {
            return this.getWorld().getSpawnLocation();
        }
        final World world = this.server.getWorld(String.valueOf(target.get(0)));
        return new Location(world, ((Number) target.get(1)).intValue(), ((Number) target.get(2)).intValue(), ((Number) target.get(3)).intValue(),
            target.size() > 4 ? ((Number) target.get(4)).floatValue() : 0f, 0f);
    }

    @Override
    public void setCompassTarget(final Location loc) {
        com.google.common.base.Preconditions.checkArgument(loc != null, "Location cannot be null");
        final World world = loc.getWorld() != null ? loc.getWorld() : this.getWorld();
        this.set("compass_target", List.of(world.getName(), loc.getBlockX(), loc.getBlockY(), loc.getBlockZ(), (double) loc.getYaw()));
    }

    private @Nullable Location respawnBlock() {
        final List<Object> respawn = Values.list(this.state.get("respawn"));
        if (respawn.size() < 4) {
            return null;
        }
        final World world = this.server.getWorld(String.valueOf(respawn.get(0)));
        return world == null ? null : new Location(world, ((Number) respawn.get(1)).intValue(), ((Number) respawn.get(2)).intValue(), ((Number) respawn.get(3)).intValue());
    }

    @Override
    public @Nullable Location getPotentialRespawnLocation() {
        return this.respawnBlock();
    }

    @Override
    public @Nullable Location getRespawnLocation(final boolean loadLocationAndValidate) {
        final Location block = this.respawnBlock();
        if (block == null || !loadLocationAndValidate) {
            return block;
        }
        // ServerPlayer.findRespawnAndUseSpawnBlock: the bed or anchor must still be there; the
        // player stands up next to it (BedBlock.findStandUpPosition, simplified).
        final org.bukkit.Material type = block.getBlock().getType();
        if (!org.bukkit.Tag.BEDS.isTagged(type) && type != org.bukkit.Material.RESPAWN_ANCHOR) {
            return null;
        }
        final int[][] around = {{1, 0}, {-1, 0}, {0, 1}, {0, -1}, {1, 1}, {1, -1}, {-1, 1}, {-1, -1}};
        for (final int dy : new int[] {0, 1, -1}) {
            for (final int[] d : around) {
                final org.bukkit.block.Block feet = block.getBlock().getRelative(d[0], dy, d[1]);
                if (feet.isPassable() && feet.getRelative(0, 1, 0).isPassable() && feet.getRelative(0, -1, 0).getType().isSolid()) {
                    return feet.getLocation().add(0.5, 0, 0.5);
                }
            }
        }
        return block.clone().add(0.5, 0.5625, 0.5);
    }

    @Override
    public void setRespawnLocation(final @Nullable Location location, final boolean force) {
        if (location == null) {
            this.set("respawn", List.of());
            return;
        }
        final World world = location.getWorld() != null ? location.getWorld() : this.getWorld();
        this.set("respawn", List.of(world.getName(), location.getBlockX(), location.getBlockY(), location.getBlockZ()));
    }

    // inventory

    @Override
    public void updateInventory() {
        this.server.send("player.update_inventory", this.args());
    }

    private static String cooldownGroup(final org.bukkit.Material material) {
        return material.getKey().toString();
    }

    @Override
    public boolean hasCooldown(final org.bukkit.Material material) {
        return this.getCooldown(material) > 0;
    }

    @Override
    public int getCooldown(final org.bukkit.Material material) {
        com.google.common.base.Preconditions.checkArgument(material != null, "Material cannot be null");
        return this.cooldown(cooldownGroup(material));
    }

    @Override
    public void setCooldown(final org.bukkit.Material material, final int ticks) {
        com.google.common.base.Preconditions.checkArgument(material != null, "Material cannot be null");
        com.google.common.base.Preconditions.checkArgument(material.isItem(), "Material %s is not an item", material);
        this.setCooldown(material.key(), ticks);
    }

    @Override
    public boolean hasCooldown(final org.bukkit.inventory.ItemStack item) {
        return this.getCooldown(item) > 0;
    }

    @Override
    public int getCooldown(final org.bukkit.inventory.ItemStack item) {
        return this.cooldown(cooldownGroup(item.getType()));
    }

    @Override
    public void setCooldown(final org.bukkit.inventory.ItemStack item, final int ticks) {
        this.setCooldown(item.getType().key(), ticks);
    }

    @Override
    public int getCooldown(final net.kyori.adventure.key.Key key) {
        return this.cooldown(key.asString());
    }

    @Override
    public void setCooldown(final net.kyori.adventure.key.Key key, final int ticks) {
        com.google.common.base.Preconditions.checkArgument(ticks >= 0, "Cannot have negative cooldown");
        this.server.call("player.cooldown", this.args("group", key.asString(), "ticks", ticks));
    }

    private int cooldown(final String group) {
        return ((Number) this.server.call("player.cooldown", this.args("group", group))).intValue();
    }

    // attack strength

    private double attackSpeed() {
        final org.bukkit.attribute.AttributeInstance speed = this.getAttribute(org.bukkit.attribute.Attribute.ATTACK_SPEED);
        return speed == null ? 4.0 : speed.getValue();
    }

    @Override
    public float getCooldownPeriod() {
        return (float) (1.0 / this.attackSpeed() * 20.0);
    }

    @Override
    public float getCooledAttackStrength(final float adjustTicks) {
        final float ticks = Values.i32(this.state, "ticks_since_swing", 100) + adjustTicks;
        return Math.max(0f, Math.min(1f, ticks / this.getCooldownPeriod()));
    }

    @Override
    public float getAttackCooldown() {
        return this.getCooledAttackStrength(0.5f);
    }

    @Override
    public void resetCooldown() {
        this.set("reset_attack_cooldown", true);
    }

    // using items

    private @Nullable List<Object> usingItem() {
        final List<Object> using = Values.list(this.state.get("using_item"));
        return using.size() < 2 ? null : using;
    }

    @Override
    public boolean isHandRaised() {
        return this.usingItem() != null;
    }

    @Override
    public org.bukkit.inventory.EquipmentSlot getHandRaised() {
        final List<Object> using = this.usingItem();
        return using != null && ((Number) using.get(0)).intValue() == 1 ? org.bukkit.inventory.EquipmentSlot.OFF_HAND : org.bukkit.inventory.EquipmentSlot.HAND;
    }

    @Override
    public org.bukkit.inventory.ItemStack getActiveItem() {
        if (!this.isHandRaised()) {
            return org.bukkit.inventory.ItemStack.empty();
        }
        return this.getInventory().getItem(this.getHandRaised()).clone();
    }

    @Override
    public org.bukkit.inventory.@Nullable ItemStack getItemInUse() {
        return this.isHandRaised() ? this.getActiveItem() : null;
    }

    @Override
    public int getActiveItemUsedTime() {
        final List<Object> using = this.usingItem();
        return using == null ? 0 : ((Number) using.get(1)).intValue();
    }

    @Override
    public void clearActiveItem() {
        this.set("clear_active_item", true);
    }

    @Override
    public org.bukkit.inventory.MainHand getMainHand() {
        return "LEFT".equals(this.state.get("main_hand")) ? org.bukkit.inventory.MainHand.LEFT : org.bukkit.inventory.MainHand.RIGHT;
    }

    // seeing other players and entities (CraftPlayer.hideEntity0 / showEntity0)

    /** Entities this player does not see, and the plugins hiding each (null: the old API). */
    private final Map<UUID, Set<@Nullable Plugin>> hiddenBy = new java.util.HashMap<>();

    private void hide(final @Nullable Plugin plugin, final Entity entity) {
        com.google.common.base.Preconditions.checkArgument(entity != null, "Entity hidden cannot be null");
        if (entity.equals(this)) {
            return;
        }
        final Set<@Nullable Plugin> by = this.hiddenBy.computeIfAbsent(entity.getUniqueId(), u -> new java.util.HashSet<>());
        final boolean first = by.isEmpty();
        by.add(plugin);
        if (first) {
            this.server.call("player.hide_entity", this.args("target", entity.getUniqueId().toString()));
            this.server.getPluginManager().callEvent(new org.bukkit.event.player.PlayerHideEntityEvent(this, entity));
        }
    }

    private void show(final @Nullable Plugin plugin, final Entity entity) {
        com.google.common.base.Preconditions.checkArgument(entity != null, "Entity show cannot be null");
        final Set<@Nullable Plugin> by = this.hiddenBy.get(entity.getUniqueId());
        if (by == null || !by.remove(plugin) || !by.isEmpty()) {
            return;
        }
        this.hiddenBy.remove(entity.getUniqueId());
        this.server.call("player.show_entity", this.args("target", entity.getUniqueId().toString()));
        this.server.getPluginManager().callEvent(new org.bukkit.event.player.PlayerShowEntityEvent(this, entity));
    }

    @Override
    public void hidePlayer(final Player player) {
        this.hide(null, player);
    }

    @Override
    public void showPlayer(final Player player) {
        this.show(null, player);
    }

    @Override
    public void showPlayer(final Plugin plugin, final Player player) {
        this.show(plugin, player);
    }

    @Override
    public void hideEntity(final Plugin plugin, final Entity entity) {
        com.google.common.base.Preconditions.checkArgument(plugin != null, "Plugin cannot be null");
        com.google.common.base.Preconditions.checkArgument(plugin.isEnabled(), "Plugin (%s) cannot be disabled", plugin.getName());
        this.hide(plugin, entity);
    }

    @Override
    public void showEntity(final Plugin plugin, final Entity entity) {
        com.google.common.base.Preconditions.checkArgument(plugin != null, "Plugin cannot be null");
        this.show(plugin, entity);
    }

    @Override
    public boolean canSee(final Player player) {
        return this.canSee((Entity) player);
    }

    @Override
    public boolean canSee(final Entity entity) {
        return !this.hiddenBy.containsKey(entity.getUniqueId());
    }

    // titles, tab list, sounds

    @Override
    public void sendTitle(final com.destroystokyo.paper.Title title) {
        com.google.common.base.Preconditions.checkNotNull(title, "Title is null");
        this.setTitleTimes(title.getFadeIn(), title.getStay(), title.getFadeOut());
        if (title.getSubtitle() != null) {
            this.setSubtitle(title.getSubtitle());
        }
        this.server.send("player.send_title", this.args("title", Components.toJson(bungee(title.getTitle()))));
    }

    @Override
    public void updateTitle(final com.destroystokyo.paper.Title title) {
        this.sendTitle(title);
    }

    @Override
    public void hideTitle() {
        this.clearTitle();
    }

    @Override
    public void setTitleTimes(final int fadeInTicks, final int stayTicks, final int fadeOutTicks) {
        this.server.send("player.send_title", this.args("fade_in", fadeInTicks, "stay", stayTicks, "fade_out", fadeOutTicks));
    }

    @Override
    public void setSubtitle(final net.md_5.bungee.api.chat.BaseComponent[] subtitle) {
        this.server.send("player.send_title", this.args("subtitle", Components.toJson(bungee(subtitle))));
    }

    @Override
    public void setSubtitle(final net.md_5.bungee.api.chat.BaseComponent subtitle) {
        this.setSubtitle(new net.md_5.bungee.api.chat.BaseComponent[] {subtitle});
    }

    private static Component bungee(final net.md_5.bungee.api.chat.BaseComponent @Nullable [] components) {
        if (components == null || components.length == 0) {
            return Component.empty();
        }
        return net.kyori.adventure.text.serializer.gson.GsonComponentSerializer.gson().deserialize(net.md_5.bungee.chat.ComponentSerializer.toString(components));
    }

    @Override
    public @Nullable String getPlayerListHeader() {
        return this.listHeader.equals(Component.empty()) ? null : Components.toLegacy(this.listHeader);
    }

    @Override
    public @Nullable String getPlayerListFooter() {
        return this.listFooter.equals(Component.empty()) ? null : Components.toLegacy(this.listFooter);
    }

    @Override
    public void setPlayerListHeader(final @Nullable String header) {
        this.sendPlayerListHeaderAndFooter(header == null ? Component.empty() : Components.legacy(header), this.listFooter);
    }

    @Override
    public void setPlayerListFooter(final @Nullable String footer) {
        this.sendPlayerListHeaderAndFooter(this.listHeader, footer == null ? Component.empty() : Components.legacy(footer));
    }

    @Override
    public void setPlayerListHeaderFooter(final @Nullable String header, final @Nullable String footer) {
        this.sendPlayerListHeaderAndFooter(header == null ? Component.empty() : Components.legacy(header), footer == null ? Component.empty() : Components.legacy(footer));
    }

    @Override
    public void setPlayerListHeaderFooter(final net.md_5.bungee.api.chat.BaseComponent @Nullable [] header, final net.md_5.bungee.api.chat.BaseComponent @Nullable [] footer) {
        this.sendPlayerListHeaderAndFooter(bungee(header), bungee(footer));
    }

    @Override
    public void setPlayerListHeaderFooter(final net.md_5.bungee.api.chat.@Nullable BaseComponent header, final net.md_5.bungee.api.chat.@Nullable BaseComponent footer) {
        this.setPlayerListHeaderFooter(header == null ? null : new net.md_5.bungee.api.chat.BaseComponent[] {header},
            footer == null ? null : new net.md_5.bungee.api.chat.BaseComponent[] {footer});
    }

    /** Particles only this player sees. */
    @Override
    public <T> void spawnParticle(final org.bukkit.Particle particle, final double x, final double y, final double z, final int count,
                                  final double offsetX, final double offsetY, final double offsetZ, final double speedX, final double speedY, final double speedZ,
                                  final @Nullable T data, final boolean force, final org.bukkit.Particle.RandomizationType randomization) {
        this.server.call("world.particles", Values.of("world", this.getWorld().getName(), "particle", particle.getKey().asString(),
            "data", io.papermc.paperrs.host.world.HostWorld.particleData(particle, data), "x", x, "y", y, "z", z, "count", count,
            "offset_x", offsetX, "offset_y", offsetY, "offset_z", offsetZ, "speed_x", speedX, "speed_y", speedY, "speed_z", speedZ,
            "force", force, "randomization", randomization.ordinal(), "to", List.of(this.uuid.toString())));
    }

    private static String soundKey(final org.bukkit.Sound sound) {
        return ((net.kyori.adventure.key.Keyed) sound).key().asString();
    }

    private void sound(final double x, final double y, final double z, final String sound, final org.bukkit.SoundCategory category, final float volume, final float pitch, final @Nullable Long seed) {
        final Map<String, Object> args = this.args("sound", sound, "category", category.ordinal(), "x", x, "y", y, "z", z, "volume", (double) volume, "pitch", (double) pitch);
        if (seed != null) {
            args.put("seed", seed);
        }
        this.server.send("player.play_sound", args);
    }

    @Override
    public void playSound(final Location location, final String sound, final org.bukkit.SoundCategory category, final float volume, final float pitch) {
        if (location == null || sound == null || category == null) {
            return;
        }
        this.sound(location.getX(), location.getY(), location.getZ(), sound, category, volume, pitch, null);
    }

    @Override
    public void playSound(final Location location, final String sound, final org.bukkit.SoundCategory category, final float volume, final float pitch, final long seed) {
        if (location == null || sound == null || category == null) {
            return;
        }
        this.sound(location.getX(), location.getY(), location.getZ(), sound, category, volume, pitch, seed);
    }

    @Override
    public void playSound(final Location location, final org.bukkit.Sound sound, final org.bukkit.SoundCategory category, final float volume, final float pitch) {
        if (location == null || sound == null || category == null) {
            return;
        }
        this.sound(location.getX(), location.getY(), location.getZ(), soundKey(sound), category, volume, pitch, null);
    }

    @Override
    public void playSound(final Location location, final org.bukkit.Sound sound, final org.bukkit.SoundCategory category, final float volume, final float pitch, final long seed) {
        if (location == null || sound == null || category == null) {
            return;
        }
        this.sound(location.getX(), location.getY(), location.getZ(), soundKey(sound), category, volume, pitch, seed);
    }

    @Override
    public void playSound(final Entity entity, final String sound, final org.bukkit.SoundCategory category, final float volume, final float pitch) {
        if (entity == null || sound == null || category == null) {
            return;
        }
        this.playSound(entity.getLocation(), sound, category, volume, pitch);
    }

    @Override
    public void playSound(final Entity entity, final String sound, final org.bukkit.SoundCategory category, final float volume, final float pitch, final long seed) {
        if (entity == null || sound == null || category == null) {
            return;
        }
        this.playSound(entity.getLocation(), sound, category, volume, pitch, seed);
    }

    @Override
    public void playSound(final Entity entity, final org.bukkit.Sound sound, final org.bukkit.SoundCategory category, final float volume, final float pitch) {
        if (entity == null || sound == null || category == null) {
            return;
        }
        this.playSound(entity.getLocation(), sound, category, volume, pitch);
    }

    @Override
    public void playSound(final Entity entity, final org.bukkit.Sound sound, final org.bukkit.SoundCategory category, final float volume, final float pitch, final long seed) {
        if (entity == null || sound == null || category == null) {
            return;
        }
        this.playSound(entity.getLocation(), sound, category, volume, pitch, seed);
    }

    @Override
    public void stopSound(final String sound, final org.bukkit.@Nullable SoundCategory category) {
        final Map<String, Object> args = this.args("sound", sound);
        if (category != null) {
            args.put("category", category.ordinal());
        }
        this.server.send("player.stop_sound", args);
    }

    @Override
    public void stopSound(final org.bukkit.SoundCategory category) {
        this.server.send("player.stop_sound", this.args("category", category.ordinal()));
    }

    @Override
    public void swingHand(final org.bukkit.inventory.EquipmentSlot hand, final io.papermc.paper.datacomponent.item.SwingAnimation animation) {
        com.google.common.base.Preconditions.checkArgument(hand == org.bukkit.inventory.EquipmentSlot.HAND || hand == org.bukkit.inventory.EquipmentSlot.OFF_HAND, "Unknown hand: %s", hand);
        this.server.send("player.swing", this.args("hand", hand == org.bukkit.inventory.EquipmentSlot.OFF_HAND ? 1 : 0));
    }

    // packets for this client only

    @Override
    public void sendHurtAnimation(final float yaw) {
        this.server.send("player.hurt_animation", this.args("yaw", (double) yaw));
    }

    @Override
    public void playHurtAnimation(final float yaw) {
        this.server.send("player.hurt_animation", this.args("yaw", (double) yaw, "everyone", true));
    }

    @Override
    public void sendBlockDamage(final Location loc, final float progress, final int sourceId) {
        com.google.common.base.Preconditions.checkArgument(loc != null, "loc must not be null");
        com.google.common.base.Preconditions.checkArgument(progress >= 0.0 && progress <= 1.0, "progress must be between 0.0 and 1.0 (inclusive)");
        // There are 0 - 9 damage states; -1 removes the overlay.
        final int stage = progress == 0 ? -1 : (int) (9 * progress);
        this.server.send("player.block_damage", this.args("x", loc.getBlockX(), "y", loc.getBlockY(), "z", loc.getBlockZ(), "stage", stage, "source", sourceId));
    }

    @Override
    public void sendBlockDamage(final Location loc, final float progress, final Entity source) {
        com.google.common.base.Preconditions.checkArgument(source != null, "source must not be null");
        this.sendBlockDamage(loc, progress, source.getEntityId());
    }

    private static int equipmentSlotId(final org.bukkit.inventory.EquipmentSlot slot) {
        return switch (slot) {
            case HAND -> 0;
            case OFF_HAND -> 1;
            case FEET -> 2;
            case LEGS -> 3;
            case CHEST -> 4;
            case HEAD -> 5;
            case BODY -> 6;
            case SADDLE -> 7;
        };
    }

    @Override
    public void sendEquipmentChange(final org.bukkit.entity.LivingEntity entity, final org.bukkit.inventory.EquipmentSlot slot, final org.bukkit.inventory.@Nullable ItemStack item) {
        this.sendEquipmentChange(entity, java.util.Collections.singletonMap(slot, item));
    }

    @Override
    public void sendEquipmentChange(final org.bukkit.entity.LivingEntity entity, final Map<org.bukkit.inventory.EquipmentSlot, org.bukkit.inventory.ItemStack> items) {
        com.google.common.base.Preconditions.checkArgument(entity != null, "Entity cannot be null");
        com.google.common.base.Preconditions.checkArgument(items != null, "items cannot be null");
        final List<Object> equipment = new java.util.ArrayList<>();
        for (final Map.Entry<org.bukkit.inventory.EquipmentSlot, org.bukkit.inventory.ItemStack> e : items.entrySet()) {
            final Map<String, Object> value = io.papermc.paperrs.host.item.HostItemStack.toValue(e.getValue());
            equipment.add(java.util.Arrays.asList(equipmentSlotId(e.getKey()), value));
        }
        this.server.send("player.equipment_change", this.args("entity", entity.getEntityId(), "equipment", equipment));
    }

    @Override
    public void showDemoScreen() {
        this.server.send("player.game_event", this.args("event", 5, "value", 0.0));
    }

    @Override
    public void showElderGuardian(final boolean silent) {
        this.server.send("player.game_event", this.args("event", 10, "value", silent ? 0.0 : 1.0));
    }

    @Override
    public void showWinScreen() {
        this.server.send("player.game_event", this.args("event", 4, "value", 1.0));
    }

    @Override
    public boolean hasSeenWinScreen() {
        return Values.bool(this.state, "seen_credits", false);
    }

    @Override
    public void setHasSeenWinScreen(final boolean hasSeenWinScreen) {
        this.set("seen_credits", hasSeenWinScreen);
    }

    @Override
    public void transfer(final String host, final int port) {
        this.server.send("player.transfer", this.args("host", host, "port", port));
    }

    private @Nullable Entity spectatorTarget;

    @Override
    public @Nullable Entity getSpectatorTarget() {
        return this.spectatorTarget;
    }

    @Override
    public void setSpectatorTarget(final @Nullable Entity entity) {
        com.google.common.base.Preconditions.checkArgument(this.getGameMode() == GameMode.SPECTATOR, "Player must be in spectator mode");
        this.spectatorTarget = entity;
        this.server.send("player.camera", this.args("target", entity == null ? this.entityId : entity.getEntityId()));
    }

    // client options

    @Override
    @SuppressWarnings("unchecked")
    public <T> T getClientOption(final com.destroystokyo.paper.ClientOption<T> option) {
        final Object value;
        if (option == com.destroystokyo.paper.ClientOption.LOCALE) {
            value = Values.str(this.state, "locale", "en_us");
        } else if (option == com.destroystokyo.paper.ClientOption.VIEW_DISTANCE) {
            value = this.getClientViewDistance();
        } else if (option == com.destroystokyo.paper.ClientOption.CHAT_VISIBILITY) {
            final int mode = Values.i32(this.state, "chat_visibility", 0);
            value = switch (mode) {
                case 0 -> com.destroystokyo.paper.ClientOption.ChatVisibility.FULL;
                case 1 -> com.destroystokyo.paper.ClientOption.ChatVisibility.SYSTEM;
                case 2 -> com.destroystokyo.paper.ClientOption.ChatVisibility.HIDDEN;
                default -> com.destroystokyo.paper.ClientOption.ChatVisibility.UNKNOWN;
            };
        } else if (option == com.destroystokyo.paper.ClientOption.CHAT_COLORS_ENABLED) {
            value = Values.bool(this.state, "chat_colors", true);
        } else if (option == com.destroystokyo.paper.ClientOption.SKIN_PARTS) {
            final int raw = Values.i32(this.state, "skin_parts", 0x7F);
            value = new com.destroystokyo.paper.SkinParts() {
                @Override public boolean hasCapeEnabled() { return (raw & 0x01) != 0; }
                @Override public boolean hasJacketEnabled() { return (raw & 0x02) != 0; }
                @Override public boolean hasLeftSleeveEnabled() { return (raw & 0x04) != 0; }
                @Override public boolean hasRightSleeveEnabled() { return (raw & 0x08) != 0; }
                @Override public boolean hasLeftPantsEnabled() { return (raw & 0x10) != 0; }
                @Override public boolean hasRightPantsEnabled() { return (raw & 0x20) != 0; }
                @Override public boolean hasHatsEnabled() { return (raw & 0x40) != 0; }
                @Override public int getRaw() { return raw; }
                @Override public com.destroystokyo.paper.SkinParts.Mutable mutableCopy() {
                    final com.destroystokyo.paper.SkinParts.Mutable copy = com.destroystokyo.paper.SkinParts.allParts();
                    copy.setCapeEnabled(this.hasCapeEnabled());
                    copy.setJacketEnabled(this.hasJacketEnabled());
                    copy.setLeftSleeveEnabled(this.hasLeftSleeveEnabled());
                    copy.setRightSleeveEnabled(this.hasRightSleeveEnabled());
                    copy.setLeftPantsEnabled(this.hasLeftPantsEnabled());
                    copy.setRightPantsEnabled(this.hasRightPantsEnabled());
                    copy.setHatsEnabled(this.hasHatsEnabled());
                    return copy;
                }
            };
        } else if (option == com.destroystokyo.paper.ClientOption.MAIN_HAND) {
            value = this.getMainHand();
        } else if (option == com.destroystokyo.paper.ClientOption.TEXT_FILTERING_ENABLED) {
            value = Values.bool(this.state, "text_filtering", false);
        } else if (option == com.destroystokyo.paper.ClientOption.ALLOW_SERVER_LISTINGS) {
            value = this.isAllowingServerListings();
        } else if (option == com.destroystokyo.paper.ClientOption.PARTICLE_VISIBILITY) {
            value = com.destroystokyo.paper.ClientOption.ParticleVisibility.values()[Math.clamp(Values.i32(this.state, "particle_status", 0), 0, 2)];
        } else {
            throw new IllegalArgumentException("Unknown client option " + option);
        }
        return (T) value;
    }

    @Override
    public boolean isAllowingServerListings() {
        return Values.bool(this.state, "allow_server_listings", true);
    }

    @Override
    public int getProtocolVersion() {
        return io.papermc.paperrs.host.HostVersions.PROTOCOL_VERSION;
    }

    @Override
    public @Nullable InetSocketAddress getHAProxyAddress() {
        return null;
    }

    @Override
    public java.util.Collection<org.bukkit.entity.EnderPearl> getEnderPearls() {
        return List.of();
    }

    // where the player looks (CraftLivingEntity.getLineOfSight, with Bukkit's BlockIterator)

    private List<org.bukkit.block.Block> lineOfSight(final @Nullable Set<org.bukkit.Material> transparent, int maxDistance, final int maxLength) {
        final Set<org.bukkit.Material> see = transparent != null ? transparent : Set.of(org.bukkit.Material.AIR, org.bukkit.Material.CAVE_AIR, org.bukkit.Material.VOID_AIR);
        if (maxDistance > 120) {
            maxDistance = 120;
        }
        final java.util.ArrayList<org.bukkit.block.Block> blocks = new java.util.ArrayList<>();
        final java.util.Iterator<org.bukkit.block.Block> itr = new org.bukkit.util.BlockIterator(this, maxDistance);
        while (itr.hasNext()) {
            final org.bukkit.block.Block block = itr.next();
            blocks.add(block);
            if (maxLength != 0 && blocks.size() > maxLength) {
                blocks.removeFirst();
            }
            if (!see.contains(block.getType())) {
                break;
            }
        }
        return blocks;
    }

    @Override
    public List<org.bukkit.block.Block> getLineOfSight(final @Nullable Set<org.bukkit.Material> transparent, final int maxDistance) {
        return this.lineOfSight(transparent, maxDistance, 0);
    }

    @Override
    public org.bukkit.block.Block getTargetBlock(final @Nullable Set<org.bukkit.Material> transparent, final int maxDistance) {
        final List<org.bukkit.block.Block> blocks = this.lineOfSight(transparent, maxDistance, 1);
        return blocks.getFirst();
    }

    @Override
    public List<org.bukkit.block.Block> getLastTwoTargetBlocks(final @Nullable Set<org.bukkit.Material> transparent, final int maxDistance) {
        return this.lineOfSight(transparent, maxDistance, 2);
    }

    /** No border of the player's own (CraftPlayer's {@code clientWorldBorder}) is supported yet. */
    @Override
    public org.bukkit.@Nullable WorldBorder getWorldBorder() {
        return null;
    }

    @Override
    public io.papermc.paper.threadedregions.scheduler.EntityScheduler getScheduler() {
        return this.scheduler;
    }

    private final HostEntityScheduler scheduler = new HostEntityScheduler(this);

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
