package org.bukkit.craftbukkit.entity;

import io.papermc.paperrs.host.stub.AbstractPlayer;
import org.bukkit.permissions.PermissibleBase;

/**
 * Where CraftBukkit keeps a player's permissions: the field {@code perm}. Permission plugins
 * put their own permissible in its place by reflection (LuckPerms' {@code PermissibleInjector}
 * looks up {@code org.bukkit.craftbukkit.entity.CraftHumanEntity#perm}), so the host's players
 * keep theirs in a field of that name, on a class of that name, and always check through it.
 */
public abstract class CraftHumanEntity extends AbstractPlayer {
    /** Set by the player once its state is known; replaced by permission plugins. */
    protected PermissibleBase perm;
}
