package io.papermc.paperrs.host.command;

import com.mojang.brigadier.LiteralMessage;
import com.mojang.brigadier.exceptions.CommandSyntaxException;
import com.mojang.brigadier.exceptions.SimpleCommandExceptionType;
import io.papermc.paper.command.brigadier.CommandSourceStack;
import net.kyori.adventure.text.ComponentLike;
import net.kyori.adventure.text.format.NamedTextColor;
import org.bukkit.Location;
import org.bukkit.command.CommandSender;
import org.bukkit.entity.Entity;
import org.bukkit.entity.Player;
import org.jetbrains.annotations.Nullable;

/** Who runs a Brigadier command, and where. */
public record HostCommandSourceStack(CommandSender sender, Location location, @Nullable Entity executor) implements CommandSourceStack {
    private static final SimpleCommandExceptionType NOT_PLAYER = new SimpleCommandExceptionType(new LiteralMessage("A player is required to run this command here"));
    private static final SimpleCommandExceptionType NOT_ENTITY = new SimpleCommandExceptionType(new LiteralMessage("An entity is required to run this command here"));

    @Override
    public Location getLocation() {
        return this.location.clone();
    }

    @Override
    public CommandSender getSender() {
        return this.sender;
    }

    @Override
    public @Nullable Entity getExecutor() {
        return this.executor;
    }

    @Override
    public Player getPlayerOrThrow() throws CommandSyntaxException {
        if (this.executor instanceof Player player) {
            return player;
        }
        throw NOT_PLAYER.create();
    }

    @Override
    public Entity getEntityOrThrow() throws CommandSyntaxException {
        if (this.executor != null) {
            return this.executor;
        }
        throw NOT_ENTITY.create();
    }

    @Override
    public CommandSourceStack withLocation(final Location location) {
        return new HostCommandSourceStack(this.sender, location.clone(), this.executor);
    }

    @Override
    public CommandSourceStack withExecutor(final Entity executor) {
        return new HostCommandSourceStack(this.sender, this.location, executor);
    }

    @Override
    public void sendReply(final ComponentLike message) {
        this.sender.sendMessage(message);
    }

    @Override
    public void sendSuccess(final ComponentLike message, final boolean allowInformingAdmins) {
        this.sender.sendMessage(message);
    }

    @Override
    public void sendFailure(final ComponentLike message) {
        this.sender.sendMessage(message.asComponent().colorIfAbsent(NamedTextColor.RED));
    }
}
