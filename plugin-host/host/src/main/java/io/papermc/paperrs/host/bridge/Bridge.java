package io.papermc.paperrs.host.bridge;

import java.io.BufferedInputStream;
import java.io.BufferedOutputStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.net.Socket;
import java.util.ArrayDeque;
import java.util.Deque;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicLong;
import java.util.logging.Level;
import java.util.logging.Logger;

/**
 * The host end of the plugin bridge.
 *
 * <h2>Threading: the main-thread baton</h2>
 *
 * Paper runs all plugin logic on one main thread. Here there are two processes, each with a
 * main thread, and exactly one of them runs game logic at any moment:
 *
 * <ul>
 *   <li>The Rust server's main thread owns the world. When it needs plugins (an event, a
 *   command, the per-tick scheduler run) it sends a CALL with {@link Codec#FLAG_SYNC} and
 *   waits.</li>
 *   <li>That CALL is executed on the host's main thread. Plugin code that calls back into
 *   the API sends its own sync CALL to Rust and waits; the Rust main thread, still inside its
 *   wait, executes it and answers. Calls nest like ordinary function calls across the two
 *   processes, and each side's wait loop services the other side's nested calls.</li>
 *   <li>Calls made by other host threads (async tasks, async events) are sent without the
 *   flag; Rust queues them and runs them on its main thread between ticks, so the
 *   single-threaded view of the world is never broken.</li>
 * </ul>
 *
 * Everything that arrives for the main thread (sync calls, notifications, and the answers to
 * the main thread's own calls) goes through one FIFO queue, so it is handled in the order the
 * Rust server sent it.
 */
public final class Bridge {
    private static final Logger LOGGER = Logger.getLogger("PluginBridge");

    /** Handles calls and notifications from the Rust server. */
    public interface Handler {
        /** Executes a call and returns its result (any bridge value). */
        Object call(String method, Map<String, Object> args) throws Exception;

        /** Handles a notification. */
        void notify(String method, Map<String, Object> args) throws Exception;
    }

    /** Thrown when the Rust side reports that a call failed. */
    public static final class RemoteException extends RuntimeException {
        public RemoteException(String method, Object detail) {
            super(method + ": " + detail);
        }
    }

    /** Thrown when the connection to the Rust server is gone. */
    public static final class DisconnectedException extends RuntimeException {
        public DisconnectedException(String message) {
            super(message);
        }
    }

    private sealed interface Item permits Inbound, Answer, Task, Closed {}

    private record Inbound(Codec.Frame frame) implements Item {}

    private record Answer(Codec.Frame frame) implements Item {}

    private record Task(Runnable runnable) implements Item {}

    private record Closed(String reason) implements Item {}

    private final Socket socket;
    private final DataInputStream in;
    private final OutputStream out;
    private final Object writeLock = new Object();
    /** Frames of {@link #send} are in the buffer, not written out yet. */
    private boolean held;
    private final AtomicLong nextId = new AtomicLong(1);
    private final LinkedBlockingQueue<Item> mainQueue = new LinkedBlockingQueue<>();
    /** Ids of calls the main thread is waiting for, innermost last. */
    private final Deque<Long> mainWaits = new ArrayDeque<>();
    private final Map<Long, Boolean> mainCallIds = new ConcurrentHashMap<>();
    /**
     * Answers to main-thread calls that came while a call made after them still waited: a
     * call the server got while it was busy with an event it fired at the host (a plugin
     * thread's call made it fire one) is answered before the event's own calls. They wait
     * here for their call ({@link #callOnMain}) rather than being dropped. Main thread only.
     */
    private final Map<Long, Codec.Frame> earlyAnswers = new java.util.HashMap<>();
    private final Map<Long, CompletableFuture<Object>> pending = new ConcurrentHashMap<>();
    private final ExecutorService asyncHandlers = Executors.newCachedThreadPool(r -> {
        Thread t = new Thread(r, "Plugin Bridge Async");
        t.setDaemon(true);
        return t;
    });
    private volatile Thread mainThread;
    private volatile Handler handler;
    private volatile boolean closed;
    private volatile String closeReason = "not connected";
    private long slowCallNanos = TimeUnit.SECONDS.toNanos(5);

    public Bridge(Socket socket) throws IOException {
        this.socket = socket;
        socket.setTcpNoDelay(true);
        this.in = new DataInputStream(new BufferedInputStream(socket.getInputStream(), 64 * 1024));
        this.out = new BufferedOutputStream(socket.getOutputStream(), 64 * 1024);
    }

    /** Sends HELLO and waits for WELCOME; returns the server's welcome payload. */
    public Map<String, Object> handshake(Map<String, Object> hello) throws IOException {
        this.write(Codec.Frame.hello(hello));
        Codec.Frame frame = Codec.read(this.in);
        if (frame == null || frame.kind() != Codec.WELCOME) {
            throw new IOException("the server did not answer the bridge handshake");
        }
        return Values.map(frame.value());
    }

    /**
     * Starts the reader thread. From now on {@code handler} receives calls; sync calls are
     * executed by whichever thread runs {@link #runMainLoop()}.
     */
    public void start(Handler handler) {
        this.handler = handler;
        this.closeReason = null;
        Thread reader = new Thread(this::readLoop, "Plugin Bridge Reader");
        reader.setDaemon(true);
        reader.start();
    }

    /** Runs the host main thread until the connection closes. */
    public void runMainLoop() {
        this.mainThread = Thread.currentThread();
        while (true) {
            Item item;
            try {
                this.runBeforeMainWrite();
                this.flushHeld();
                item = this.spinFor();
                if (item == null) {
                    item = this.mainQueue.take();
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;
            }
            if (item instanceof Closed) {
                return;
            }
            this.handleOnMain(item);
        }
    }

    public boolean isMainThread() {
        return Thread.currentThread() == this.mainThread;
    }

    public Thread mainThread() {
        return this.mainThread;
    }

    public boolean isClosed() {
        return this.closed;
    }

    /** Runs {@code task} on the main thread in order with everything else queued there. */
    public void runOnMain(Runnable task) {
        this.mainQueue.add(new Task(task));
    }

    /**
     * Calls a method of the Rust server and returns its result.
     *
     * <p>On the main thread this is a sync call: the Rust main thread runs it immediately,
     * and calls Rust makes back into the host meanwhile are executed here, nested. On any
     * other thread it is queued for the Rust main thread and this thread blocks until done.
     */
    public Object call(String method, Map<String, Object> args) {
        if (this.closed) {
            throw new DisconnectedException("plugin bridge closed: " + this.closeReason);
        }
        long id = this.nextId.getAndIncrement();
        if (this.isMainThread()) {
            return this.callOnMain(id, method, args);
        }
        CompletableFuture<Object> future = new CompletableFuture<>();
        this.pending.put(id, future);
        try {
            this.write(Codec.Frame.call(id, 0, method, args));
            return future.get();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new DisconnectedException("interrupted while waiting for " + method);
        } catch (ExecutionException e) {
            if (e.getCause() instanceof RuntimeException r) {
                throw r;
            }
            throw new RuntimeException(e.getCause());
        } catch (IOException e) {
            throw new DisconnectedException("plugin bridge write failed: " + e.getMessage());
        } finally {
            this.pending.remove(id);
        }
    }

    /**
     * Calls a method of the Rust server from the main thread without waiting, for calls whose
     * result plugins never see (a message, a title, a sound). The server runs it in order with
     * every other sync call and answers nothing; it only logs a failure.
     */
    public void send(String method, Map<String, Object> args) {
        if (this.closed) {
            throw new DisconnectedException("plugin bridge closed: " + this.closeReason);
        }
        this.runBeforeMainWrite();
        long id = this.nextId.getAndIncrement();
        try {
            // Written out with the next frame that is waited for, or before the main thread
            // waits (see flushHeld), so a burst of these costs one write.
            byte[] bytes = Codec.encode(Codec.Frame.call(id, Codec.FLAG_SYNC | Codec.FLAG_NO_REPLY, method, args));
            synchronized (this.writeLock) {
                this.out.write(bytes);
                this.held = true;
            }
        } catch (IOException e) {
            throw new DisconnectedException("plugin bridge write failed: " + e.getMessage());
        }
    }

    /** How long the main thread looks busily for work before it sleeps. */
    private static final long SPIN_NANOS = TimeUnit.MICROSECONDS.toNanos(150);

    /**
     * The next item for the main thread if one comes within {@link #SPIN_NANOS}, looked for
     * busily: the server's calls come in bursts (the events of a tick), and waking a sleeping
     * thread takes about as long as the call itself.
     */
    private Item spinFor() {
        final long until = System.nanoTime() + SPIN_NANOS;
        do {
            final Item item = this.mainQueue.poll();
            if (item != null) {
                return item;
            }
            Thread.onSpinWait();
        } while (System.nanoTime() < until);
        return null;
    }

    /** Runs on the main thread before anything it sends (see {@link #beforeMainWrite}). */
    private volatile Runnable beforeMainWrite;
    /** Whether the main thread is running {@link #beforeMainWrite}, which sends too. */
    private boolean inBeforeMainWrite;

    /**
     * Sets what runs on the main thread before each frame it writes and before it waits for
     * work: the host's queued block edits go out first, so the server sees everything the main
     * thread does in the order it did it.
     */
    public void beforeMainWrite(final Runnable hook) {
        this.beforeMainWrite = hook;
    }

    /** The next frame the main thread writes skips the hook (see {@link #callAsRead}). */
    private boolean skipBeforeMainWriteOnce;

    /**
     * {@link #call} for a read the caller has prepared for itself: its frame does not run the
     * {@link #beforeMainWrite} hook (frames written while it waits do).
     */
    public Object callAsRead(final String method, final Map<String, Object> args) {
        if (!this.isMainThread()) {
            return this.call(method, args);
        }
        this.skipBeforeMainWriteOnce = true;
        try {
            return this.call(method, args);
        } finally {
            this.skipBeforeMainWriteOnce = false;
        }
    }

    private void runBeforeMainWrite() {
        final Runnable hook = this.beforeMainWrite;
        if (hook == null || this.inBeforeMainWrite || !this.isMainThread()) {
            return;
        }
        if (this.skipBeforeMainWriteOnce) {
            this.skipBeforeMainWriteOnce = false;
            return;
        }
        this.inBeforeMainWrite = true;
        try {
            hook.run();
        } finally {
            this.inBeforeMainWrite = false;
        }
    }

    /** Writes out what {@link #send} left in the buffer. */
    private void flushHeld() {
        synchronized (this.writeLock) {
            if (!this.held) {
                return;
            }
            this.held = false;
            try {
                this.out.flush();
            } catch (IOException e) {
                LOGGER.log(Level.FINE, "could not write to the server", e);
            }
        }
    }

    /** Like {@link #call} but never blocks; for fire-and-forget operations from any thread. */
    public CompletableFuture<Object> callAsync(String method, Map<String, Object> args) {
        long id = this.nextId.getAndIncrement();
        CompletableFuture<Object> future = new CompletableFuture<>();
        if (this.closed) {
            future.completeExceptionally(new DisconnectedException("plugin bridge closed: " + this.closeReason));
            return future;
        }
        this.pending.put(id, future);
        try {
            this.write(Codec.Frame.call(id, 0, method, args));
        } catch (IOException e) {
            this.pending.remove(id);
            future.completeExceptionally(new DisconnectedException("plugin bridge write failed: " + e.getMessage()));
        }
        return future;
    }

    /** Sends a notification (no answer). */
    public void notify(String method, Map<String, Object> args) {
        if (this.closed) {
            return;
        }
        try {
            this.write(Codec.Frame.notify(method, args));
        } catch (IOException e) {
            LOGGER.log(Level.FINE, "could not send " + method, e);
        }
    }

    private Object callOnMain(long id, String method, Map<String, Object> args) {
        this.mainCallIds.put(id, Boolean.TRUE);
        this.mainWaits.addLast(id);
        long started = System.nanoTime();
        try {
            this.write(Codec.Frame.call(id, Codec.FLAG_SYNC, method, args));
            while (true) {
                // The answer may have come already, while a call made meanwhile waited.
                final Codec.Frame early = this.earlyAnswers.remove(id);
                if (early != null) {
                    if (!early.ok()) {
                        throw new RemoteException(method, early.value());
                    }
                    return early.value();
                }
                Item item;
                try {
                    this.flushHeld();
                    item = this.spinFor();
                    if (item == null) {
                        item = this.mainQueue.poll(1, TimeUnit.SECONDS);
                    }
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    throw new DisconnectedException("interrupted while waiting for " + method);
                }
                if (item == null) {
                    if (System.nanoTime() - started > this.slowCallNanos) {
                        LOGGER.warning("Still waiting for the server to answer " + method + " after "
                            + TimeUnit.NANOSECONDS.toSeconds(System.nanoTime() - started) + "s");
                        started = System.nanoTime();
                    }
                    continue;
                }
                if (item instanceof Closed c) {
                    // Leave the marker for the outer loop.
                    this.mainQueue.add(c);
                    throw new DisconnectedException("plugin bridge closed: " + c.reason());
                }
                if (item instanceof Answer a && a.frame().id() == id) {
                    if (!a.frame().ok()) {
                        throw new RemoteException(method, a.frame().value());
                    }
                    return a.frame().value();
                }
                // The answer to a call this one was made inside of (see earlyAnswers).
                if (item instanceof Answer a && this.mainCallIds.containsKey(a.frame().id())) {
                    this.earlyAnswers.put(a.frame().id(), a.frame());
                    continue;
                }
                this.handleOnMain(item);
            }
        } catch (IOException e) {
            throw new DisconnectedException("plugin bridge write failed: " + e.getMessage());
        } finally {
            this.mainWaits.removeLast();
            this.mainCallIds.remove(id);
            this.earlyAnswers.remove(id);
        }
    }

    private void handleOnMain(Item item) {
        switch (item) {
            case Inbound inbound -> this.dispatch(inbound.frame());
            case Task task -> {
                try {
                    task.runnable().run();
                } catch (Throwable t) {
                    LOGGER.log(Level.SEVERE, "Unhandled exception in a main-thread task", t);
                }
            }
            case Answer answer -> LOGGER.warning("Dropping an answer to call " + answer.frame().id() + " that nobody waits for");
            case Closed closed -> this.mainQueue.add(closed);
        }
    }

    /** Executes an inbound CALL or NOTIFY on the current thread and answers a CALL. */
    private void dispatch(Codec.Frame frame) {
        Map<String, Object> args = frame.value() == null ? Map.of() : Values.map(frame.value());
        if (frame.kind() == Codec.NOTIFY) {
            try {
                this.handler.notify(frame.method(), args);
            } catch (Throwable t) {
                LOGGER.log(Level.SEVERE, "Error handling server notification " + frame.method(), t);
            }
            return;
        }
        Object result;
        boolean ok;
        try {
            result = this.handler.call(frame.method(), args);
            ok = true;
        } catch (Throwable t) {
            if (t instanceof DisconnectedException) {
                return;
            }
            LOGGER.log(Level.SEVERE, "Error handling server call " + frame.method(), t);
            result = t.getClass().getSimpleName() + ": " + t.getMessage();
            ok = false;
        }
        try {
            this.write(Codec.Frame.ret(frame.id(), ok, result));
        } catch (IOException e) {
            LOGGER.log(Level.FINE, "could not answer " + frame.method(), e);
        } catch (IllegalArgumentException e) {
            LOGGER.log(Level.SEVERE, "Result of " + frame.method() + " cannot be encoded", e);
            try {
                this.write(Codec.Frame.ret(frame.id(), false, e.getMessage()));
            } catch (IOException ignored) {
                // connection is going away
            }
        }
    }

    private void readLoop() {
        String reason = "connection closed by the server";
        try {
            while (true) {
                Codec.Frame frame = Codec.read(this.in);
                if (frame == null) {
                    break;
                }
                switch (frame.kind()) {
                    case Codec.RETURN -> {
                        if (this.mainCallIds.containsKey(frame.id())) {
                            this.mainQueue.add(new Answer(frame));
                        } else {
                            CompletableFuture<Object> future = this.pending.remove(frame.id());
                            if (future == null) {
                                LOGGER.warning("Answer for unknown call " + frame.id());
                            } else if (frame.ok()) {
                                future.complete(frame.value());
                            } else {
                                future.completeExceptionally(new RemoteException("call " + frame.id(), frame.value()));
                            }
                        }
                    }
                    case Codec.CALL -> {
                        if ((frame.flags() & Codec.FLAG_SYNC) != 0) {
                            this.mainQueue.add(new Inbound(frame));
                        } else {
                            this.asyncHandlers.execute(() -> this.dispatch(frame));
                        }
                    }
                    // Notifications describe world state; apply them in order on the main thread.
                    case Codec.NOTIFY -> this.mainQueue.add(new Inbound(frame));
                    default -> LOGGER.warning("Unexpected bridge frame kind " + frame.kind());
                }
            }
        } catch (IOException | RuntimeException e) {
            reason = e.getMessage();
            if (!this.closed) {
                LOGGER.log(Level.SEVERE, "Plugin bridge failed", e);
            }
        } finally {
            this.closed = true;
            this.closeReason = reason;
            for (CompletableFuture<Object> future : this.pending.values()) {
                future.completeExceptionally(new DisconnectedException("plugin bridge closed: " + reason));
            }
            this.pending.clear();
            this.mainQueue.add(new Closed(reason));
        }
    }

    private void write(Codec.Frame frame) throws IOException {
        this.runBeforeMainWrite();
        byte[] bytes = Codec.encode(frame);
        synchronized (this.writeLock) {
            this.out.write(bytes);
            this.out.flush();
            this.held = false;
        }
    }

    /** Closes the connection; the main loop ends once it has drained. */
    public void close() {
        this.closed = true;
        try {
            this.socket.close();
        } catch (IOException ignored) {
            // already closed
        }
        this.asyncHandlers.shutdownNow();
    }

    /** Waits at most {@code timeout} for a future, translating failures. */
    public static Object await(CompletableFuture<Object> future, long timeout, TimeUnit unit) throws TimeoutException {
        try {
            return future.get(timeout, unit);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new DisconnectedException("interrupted");
        } catch (ExecutionException e) {
            if (e.getCause() instanceof RuntimeException r) {
                throw r;
            }
            throw new RuntimeException(e.getCause());
        }
    }
}
