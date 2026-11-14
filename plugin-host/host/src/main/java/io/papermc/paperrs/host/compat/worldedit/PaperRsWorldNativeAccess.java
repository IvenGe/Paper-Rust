package io.papermc.paperrs.host.compat.worldedit;

import com.sk89q.worldedit.internal.wna.WorldNativeAccess;
import com.sk89q.worldedit.math.BlockVector3;
import com.sk89q.worldedit.util.SideEffect;
import com.sk89q.worldedit.util.SideEffectSet;
import com.sk89q.worldedit.world.block.BlockState;
import io.papermc.paperrs.host.data.HostData;
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
    private SideEffectSet sideEffects = SideEffectSet.defaults();

    PaperRsWorldNativeAccess(final World world) {
        this.world = world;
    }

    @Override
    public void setCurrentSideEffectSet(final SideEffectSet sideEffectSet) {
        this.sideEffects = sideEffectSet;
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
        final Object block = PaperRsAdapter.server().call("world.get_block", PaperRsAdapter.args(this.world, position.x(), position.y(), position.z()));
        return state(((Number) ((Map<?, ?>) block).get("id")).intValue());
    }

    /** Sets the block; the previous state if it changed, else null. */
    @Override
    public @Nullable State setBlockState(final Chunk chunk, final BlockVector3 position, final State state) {
        final Map<String, Object> args = PaperRsAdapter.args(this.world, position.x(), position.y(), position.z());
        args.put("state", state.id);
        args.put("physics", this.sideEffects.shouldApply(SideEffect.NEIGHBORS) || this.sideEffects.shouldApply(SideEffect.UPDATE));
        args.put("return_old", true);
        final Object old = PaperRsAdapter.server().call("world.set_block", args);
        return old instanceof Number n ? state(n.intValue()) : null;
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
        return Boolean.TRUE.equals(PaperRsAdapter.server().call("world.set_block_entity", args));
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

    @Override
    public void notifyNeighbors(final BlockVector3 pos, final State oldState, final State newState) {
        // Done by the server's physics when the block is set with them.
    }

    @Override
    public void updateNeighbors(final BlockVector3 pos, final State oldState, final State newState, final int recursionLimit) {
    }

    @Override
    public void onBlockStateChange(final BlockVector3 pos, final State oldState, final State newState) {
    }
}
