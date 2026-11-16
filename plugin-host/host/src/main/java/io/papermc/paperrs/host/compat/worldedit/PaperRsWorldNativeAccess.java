package io.papermc.paperrs.host.compat.worldedit;

import com.sk89q.worldedit.internal.wna.WorldNativeAccess;
import com.sk89q.worldedit.math.BlockVector3;
import com.sk89q.worldedit.util.SideEffect;
import com.sk89q.worldedit.util.SideEffectSet;
import com.sk89q.worldedit.world.block.BlockState;
import io.papermc.paperrs.host.data.HostData;
import io.papermc.paperrs.host.world.BlockEdits;
import io.papermc.paperrs.host.world.HostWorld;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import org.bukkit.World;
import org.enginehub.linbus.tree.LinCompoundTag;
import org.enginehub.linbus.tree.LinRootEntry;
import org.jetbrains.annotations.Nullable;

/**
 * WorldEdit's low-level block access on Paper-RS: block states by the server's state id (one
 * object per id, as WorldEdit compares them by identity), set with or without the server's
 * physics depending on the edit's side effects. The server updates light and tells players
 * itself.
 *
 * <p>An edit is many blocks: reads come from the host's section cache and changes without
 * physics (all of them in WorldEdit's usual mode, which applies the side effects after the
 * edit) are queued and sent in batches ({@link BlockEdits}), as are the side effects. Only a
 * change with physics, or side effects plugins are to hear of, is a call of its own.
 */
final class PaperRsWorldNativeAccess implements WorldNativeAccess<PaperRsWorldNativeAccess.Chunk, PaperRsWorldNativeAccess.State, BlockVector3> {
    /** A chunk by position; blocks are read through the world. */
    record Chunk(int x, int z) {
    }

    /** A block state of the server, by id. */
    static final class State {
        final int id;

        private State(final int id) {
            this.id = id;
        }
    }

    private static final State[] STATES = new State[HostData.get().stateCount()];
    private static final Map<BlockState, State> NATIVE = new ConcurrentHashMap<>();

    static State state(final int id) {
        State s = STATES[id];
        if (s == null) {
            synchronized (STATES) {
                s = STATES[id];
                if (s == null) {
                    s = new State(id);
                    STATES[id] = s;
                }
            }
        }
        return s;
    }

    private final World world;
    private final HostWorld host;
    private SideEffectSet sideEffects = SideEffectSet.defaults();
    /**
     * The change being made was set with the server's physics, which did its side effects
     * already: WorldEdit's calls for them right after are not to do them twice.
     */
    private boolean physicsDone;

    PaperRsWorldNativeAccess(final World world) {
        this.world = world;
        this.host = (HostWorld) world;
    }

    @Override
    public void setCurrentSideEffectSet(final SideEffectSet sideEffectSet) {
        // Called as each change (or each change's later side effects) begins.
        this.sideEffects = sideEffectSet;
        this.physicsDone = false;
    }

    @Override
    public Chunk getChunk(final int x, final int z) {
        return new Chunk(x, z);
    }

    @Override
    public State toNative(final BlockState state) {
        return NATIVE.computeIfAbsent(state, s -> {
            final int id = HostData.get().parseState(s.getAsString());
            if (id < 0) {
                throw new IllegalArgumentException("Paper-RS has no block state " + s.getAsString());
            }
            return state(id);
        });
    }

    @Override
    public State getBlockState(final Chunk chunk, final BlockVector3 position) {
        return state(this.host.stateId(position.x(), position.y(), position.z()));
    }

    /** Sets the block; the previous state if it changed, else null. */
    @Override
    public @Nullable State setBlockState(final Chunk chunk, final BlockVector3 position, final State state) {
        final boolean physics = this.sideEffects.shouldApply(SideEffect.NEIGHBORS) || this.sideEffects.shouldApply(SideEffect.UPDATE);
        if (!physics && PaperRsAdapter.server().isPrimaryThread()) {
            final int old = this.host.setBlockQuietly(position.x(), position.y(), position.z(), state.id);
            return old < 0 ? null : state(old);
        }
        final Map<String, Object> args = PaperRsAdapter.args(this.world, position.x(), position.y(), position.z());
        args.put("state", state.id);
        args.put("physics", physics);
        args.put("return_old", true);
        final Object old = PaperRsAdapter.server().call("world.set_block", args);
        this.physicsDone = physics;
        return old instanceof Number n ? state(n.intValue()) : null;
    }

    /**
     * Queues side effects of a change, run by the server when it gets to them; with
     * WorldEdit's {@code EVENTS} side effect a call of its own, as plugins hear of them.
     */
    private void effects(final BlockVector3 pos, final State oldState, final State newState, final int flags) {
        if (this.physicsDone) {
            return;
        }
        if (this.sideEffects.shouldApply(SideEffect.EVENTS) || !PaperRsAdapter.server().isPrimaryThread()) {
            final java.nio.ByteBuffer edit = java.nio.ByteBuffer.allocate(28).order(java.nio.ByteOrder.LITTLE_ENDIAN);
            edit.putInt(1).putInt(pos.x()).putInt(pos.y()).putInt(pos.z()).putInt(newState.id).putInt(oldState.id).putInt(flags);
            PaperRsAdapter.server().call("world.edit_batch", io.papermc.paperrs.host.bridge.Values.of("world", this.world.getName(), "edits", edit.array(), "events", true));
            return;
        }
        PaperRsAdapter.server().blockEdits().effects(this.world.getName(), pos.x(), pos.y(), pos.z(), oldState.id, newState.id, flags);
    }

    /** WorldEdit's {@code UPDATE} side effect: the block's {@code onPlace}, as on Paper. */
    @Override
    public void updateBlock(final BlockVector3 pos, final State oldState, final State newState) {
        if (oldState != newState) {
            this.effects(pos, oldState, newState, BlockEdits.ON_PLACE);
        }
    }

    @Override
    public State getValidBlockForPosition(final State block, final BlockVector3 position) {
        return block;
    }

    @Override
    public BlockVector3 getPosition(final int x, final int y, final int z) {
        return BlockVector3.at(x, y, z);
    }

    @Override
    public void updateLightingForBlock(final BlockVector3 position) {
        // The server relights changed blocks itself.
    }

    @Override
    public boolean updateTileEntity(final BlockVector3 position, final LinCompoundTag tag) {
        final Map<String, Object> args = PaperRsAdapter.args(this.world, position.x(), position.y(), position.z());
        args.put("nbt", new LinRootEntry("", tag).writeToArray());
        // A block entity changes no block states: the section cache stays good.
        return Boolean.TRUE.equals(PaperRsAdapter.server().read("world.set_block_entity", args));
    }

    @Override
    public void notifyBlockUpdate(final Chunk chunk, final BlockVector3 position, final State oldState, final State newState) {
        // Players are told about the change when it is made.
    }

    @Override
    public boolean isChunkTicking(final Chunk chunk) {
        return true;
    }

    @Override
    public void markBlockChanged(final Chunk chunk, final BlockVector3 position) {
    }

    /**
     * WorldEdit's {@code NEIGHBORS} side effect: the neighbours' updates (and comparators).
     * Done by the server's physics when the block is set with them, and after an edit here.
     */
    @Override
    public void notifyNeighbors(final BlockVector3 pos, final State oldState, final State newState) {
        this.effects(pos, oldState, newState, BlockEdits.NEIGHBORS);
    }

    /** The neighbours' shapes ({@code NEIGHBORS} too): fences, walls and stairs connect. */
    @Override
    public void updateNeighbors(final BlockVector3 pos, final State oldState, final State newState, final int recursionLimit) {
        this.effects(pos, oldState, newState, BlockEdits.SHAPES);
    }

    @Override
    public void onBlockStateChange(final BlockVector3 pos, final State oldState, final State newState) {
        // The server keeps no index of points of interest to update.
    }
}
