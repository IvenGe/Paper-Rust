package io.papermc.paperrs.host.world;

import io.papermc.paperrs.host.bridge.Values;
import org.bukkit.craftbukkit.CraftServer;
import org.jetbrains.annotations.Nullable;

/**
 * Block changes the main thread makes without waiting for the server: WorldEdit's edits and
 * Bukkit's changes without physics. Each would otherwise be a call there and back; here they
 * are packed and go out in batches ({@code world.edit_batch}) that the server answers nothing
 * to. The bridge writes the queue out before anything else the main thread sends (see
 * {@code Bridge#beforeMainWrite}), so the server sees everything in the order the host did
 * it, and before the main thread answers the server or waits for work.
 *
 * <p>A change without physics changes only its own block: the host's section cache stays
 * right by making it too ({@link HostWorld#setBlockQuietly}). Side effects (a block's
 * {@code onPlace}, its neighbours' updates) can change any block, so a batch with them moves
 * the epoch on once it is sent, and cached sections are read again.
 *
 * <p>Main thread only.
 */
public final class BlockEdits {
    static final int SET = 0;
    static final int EFFECTS = 1;
    /** The block's {@code onPlace} (WorldEdit's {@code UPDATE} side effect). */
    public static final int ON_PLACE = 1;
    /** The neighbours' {@code neighborChanged} and comparators (WorldEdit's {@code NEIGHBORS}). */
    public static final int NEIGHBORS = 2;
    /** The neighbours' shapes (fences, walls and stairs connecting; {@code NEIGHBORS} too). */
    public static final int SHAPES = 4;

    private static final int FIELDS = 7;
    /**
     * Changes per batch (about 230 KB): a full queue goes out at the next safe point
     * ({@link #flushIfFull}), so one block's side effects never straddle two batches, and in
     * any case at twice as many.
     */
    private static final int MAX = 8192;
    private static final int HARD_MAX = 2 * MAX;

    private final CraftServer server;
    private int[] buffer = new int[FIELDS * 256];
    private int count;
    private @Nullable String world;
    private boolean effects;
    /** The first queued side effects (-1: none), and whether a change was queued after them. */
    private int firstEffects = -1;
    private boolean setAfterEffects;
    private boolean flushing;
    private boolean hooked;

    public BlockEdits(final CraftServer server) {
        this.server = server;
    }

    /** Queues setting a block without physics. */
    void set(final String world, final int x, final int y, final int z, final int state) {
        this.add(world, SET, x, y, z, state, 0, 0);
    }

    /**
     * Queues side effects of a change WorldEdit made: run when the server gets to them, if the
     * block is still {@code state} then, without plugin events.
     */
    public void effects(final String world, final int x, final int y, final int z, final int old, final int state, final int flags) {
        // WorldEdit asks for each block's side effects one by one: one entry does for all.
        if (this.count > 0 && world.equals(this.world)) {
            final int i = (this.count - 1) * FIELDS;
            final int[] b = this.buffer;
            if (b[i] == EFFECTS && b[i + 1] == x && b[i + 2] == y && b[i + 3] == z && b[i + 4] == state && b[i + 5] == old) {
                b[i + 6] |= flags;
                return;
            }
        }
        this.add(world, EFFECTS, x, y, z, state, old, flags);
    }

    private void add(final String world, final int kind, final int x, final int y, final int z, final int state, final int old, final int flags) {
        if (!this.hooked) {
            this.hooked = true;
            this.server.flushBlockEditsFirst();
        }
        if (this.count > 0 && !world.equals(this.world)) {
            this.flush();
        }
        this.world = world;
        if (kind == EFFECTS) {
            if (this.firstEffects < 0) {
                this.firstEffects = this.count;
            }
            this.effects = true;
        } else if (this.firstEffects >= 0) {
            this.setAfterEffects = true;
        }
        final int i = this.count * FIELDS;
        if (i + FIELDS > this.buffer.length) {
            this.buffer = java.util.Arrays.copyOf(this.buffer, this.buffer.length * 2);
        }
        final int[] b = this.buffer;
        b[i] = kind;
        b[i + 1] = x;
        b[i + 2] = y;
        b[i + 3] = z;
        b[i + 4] = state;
        b[i + 5] = old;
        b[i + 6] = flags;
        this.count++;
        if (this.count >= HARD_MAX) {
            this.flush();
        }
    }

    /** Whether changes wait to go out. */
    public boolean isEmpty() {
        return this.count == 0;
    }

    /** Whether side effects for {@code world} wait to go out (they may change any block). */
    public boolean hasEffects(final String world) {
        return this.firstEffects >= 0 && world.equals(this.world);
    }

    /**
     * Sends the queue if it is full. Called where no block's side effects are half queued:
     * as WorldEdit starts on a block, before a Bukkit change.
     */
    public void flushIfFull() {
        if (this.count >= MAX) {
            this.flush();
        }
    }

    /** Forgets what is queued (the server is gone). */
    private void drop() {
        this.count = 0;
        this.world = null;
        this.effects = false;
        this.firstEffects = -1;
        this.setAfterEffects = false;
    }

    /** Sends what is queued. */
    public void flush() {
        if (this.count == 0 || this.flushing) {
            return;
        }
        final boolean effects = this.effects;
        this.send(this.count);
        this.world = null;
        this.effects = false;
        this.firstEffects = -1;
        this.setAfterEffects = false;
        if (effects) {
            this.server.advanceEpoch();
        }
    }

    /**
     * Sends what a read must see: the changes, but not the side effects queued after an edit
     * (WorldEdit queues them for every block it changed, reading block after block meanwhile;
     * sending them would make every cached section stale). They go out with the next call
     * that is not a read, when the main thread answers the server, or when the queue is full.
     * If a change was queued after side effects, everything goes, in order.
     */
    public void flushForRead() {
        if (this.count == 0 || this.flushing) {
            return;
        }
        if (this.firstEffects < 0 || this.setAfterEffects) {
            this.flush();
            return;
        }
        final int changes = this.firstEffects;
        if (changes == 0) {
            return;
        }
        this.send(changes);
        System.arraycopy(this.buffer, changes * FIELDS, this.buffer, 0, this.count * FIELDS);
        this.firstEffects = 0;
    }

    /** Sends the first {@code n} queued entries and takes them off the queue's count. */
    private void send(final int n) {
        this.flushing = true;
        final byte[] bytes = new byte[n * FIELDS * 4];
        for (int i = 0, m = n * FIELDS; i < m; i++) {
            final int v = this.buffer[i];
            bytes[i * 4] = (byte) v;
            bytes[i * 4 + 1] = (byte) (v >> 8);
            bytes[i * 4 + 2] = (byte) (v >> 16);
            bytes[i * 4 + 3] = (byte) (v >> 24);
        }
        this.count -= n;
        try {
            this.server.sendKnown("world.edit_batch", Values.of("world", this.world, "edits", bytes));
        } catch (final io.papermc.paperrs.host.bridge.Bridge.DisconnectedException e) {
            // The server is gone: nothing more goes out, and the host shuts down as usual.
            this.drop();
        } finally {
            this.flushing = false;
        }
    }
}
