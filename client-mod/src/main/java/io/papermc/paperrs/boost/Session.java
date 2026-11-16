package io.papermc.paperrs.boost;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import java.io.BufferedInputStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.net.SocketAddress;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.TimeUnit;
import java.util.zip.DataFormatException;
import java.util.zip.Inflater;
import javax.crypto.Cipher;
import javax.crypto.CipherInputStream;
import javax.crypto.spec.IvParameterSpec;
import javax.crypto.spec.SecretKeySpec;
import net.minecraft.client.multiplayer.ClientPacketListener;
import net.minecraft.core.RegistryAccess;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.chat.Component;
import net.minecraft.network.protocol.game.ClientboundLevelChunkWithLightPacket;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * One connection's extra streams: a thread per stream reads, decrypts, inflates and decodes
 * the chunks the server sends over it, and each chunk is handled on the client thread when its
 * marker comes on the main connection.
 */
final class Session {
    private static final Logger LOGGER = LoggerFactory.getLogger("Paper-RS Boost");
    private static final byte[] MAGIC = {'P', 'R', 'S', 'B'};
    /** How long a marker may wait for its chunk before the connection is given up. */
    private static final long CHUNK_TIMEOUT_SECONDS = 15;

    private final byte[] token;
    private final int streams;
    /** The compression threshold of the connection (-1 for none). */
    private final int threshold;
    private final Map<Integer, CompletableFuture<ClientboundLevelChunkWithLightPacket>> chunks = new ConcurrentHashMap<>();
    private final List<Socket> sockets = new ArrayList<>();
    private volatile boolean closed;

    Session(final byte[] token, final int streams, final int threshold) {
        this.token = token;
        this.streams = streams;
        this.threshold = threshold;
    }

    /** Opens the streams to the server the player just joined. */
    void open(final ClientPacketListener listener) {
        final SocketAddress address = listener.getConnection().getRemoteAddress();
        if (!(address instanceof InetSocketAddress inet)) {
            // A local connection: nothing to speed up.
            return;
        }
        final RegistryAccess registries = listener.registryAccess();
        for (int index = 0; index < this.streams; index++) {
            final int i = index;
            final Thread thread = new Thread(() -> this.read(listener, inet, registries, i), "Paper-RS Boost stream " + i);
            thread.setDaemon(true);
            thread.start();
        }
    }

    void close() {
        this.closed = true;
        synchronized (this.sockets) {
            for (final Socket s : this.sockets) {
                try {
                    s.close();
                } catch (final IOException ignored) {
                    // closing anyway
                }
            }
            this.sockets.clear();
        }
        this.chunks.values().forEach(f -> f.cancel(false));
    }

    /** A marker: the chunk with this number is handled now, waiting for it if need be. */
    void marker(final int seq, final ClientPacketListener listener) {
        final CompletableFuture<ClientboundLevelChunkWithLightPacket> pending = this.chunks.computeIfAbsent(seq, k -> new CompletableFuture<>());
        try {
            final ClientboundLevelChunkWithLightPacket packet = pending.get(CHUNK_TIMEOUT_SECONDS, TimeUnit.SECONDS);
            this.chunks.remove(seq);
            packet.handle(listener);
        } catch (final Exception e) {
            LOGGER.warn("Chunk {} never came over the extra connections", seq, e);
            listener.getConnection().disconnect(Component.literal("Paper-RS Boost: chunk data was lost"));
        }
    }

    private void read(final ClientPacketListener listener, final InetSocketAddress address, final RegistryAccess registries, final int index) {
        try (Socket socket = new Socket()) {
            synchronized (this.sockets) {
                if (this.closed) {
                    return;
                }
                this.sockets.add(socket);
            }
            socket.setTcpNoDelay(true);
            socket.connect(address, 10_000);
            final OutputStream out = socket.getOutputStream();
            final byte[] header = new byte[14];
            System.arraycopy(MAGIC, 0, header, 0, 4);
            header[4] = 1;
            System.arraycopy(digest("paper-rs:boost id", this.token, null), 0, header, 5, 8);
            header[13] = (byte) index;
            out.write(header);
            out.flush();
            final InputStream raw = socket.getInputStream();
            if (raw.read() != 1) {
                LOGGER.warn("The server refused extra connection {}", index);
                return;
            }
            final byte[] key = Arrays.copyOf(digest("paper-rs:boost stream", this.token, (byte) index), 16);
            final Cipher cipher = Cipher.getInstance("AES/CFB8/NoPadding");
            cipher.init(Cipher.DECRYPT_MODE, new SecretKeySpec(key, "AES"), new IvParameterSpec(key));
            final DataInputStream in = new DataInputStream(new BufferedInputStream(new CipherInputStream(raw, cipher), 1 << 16));
            final Inflater inflater = new Inflater();
            while (!this.closed) {
                final byte[] message = new byte[readVarInt(in)];
                in.readFully(message);
                final ByteBuf buf = Unpooled.wrappedBuffer(message);
                final int seq = readVarInt(buf);
                readVarInt(buf); // the frame's length: the rest of the message
                final ByteBuf payload = this.decompress(buf, inflater);
                readVarInt(payload); // the packet id (a chunk)
                final ClientboundLevelChunkWithLightPacket packet = ClientboundLevelChunkWithLightPacket.STREAM_CODEC.decode(new RegistryFriendlyByteBuf(payload, registries));
                this.chunks.computeIfAbsent(seq, k -> new CompletableFuture<>()).complete(packet);
            }
        } catch (final IOException | GeneralSecurityException | DataFormatException | RuntimeException e) {
            if (!this.closed) {
                LOGGER.warn("Extra connection {} failed", index, e);
                listener.getConnection().disconnect(Component.literal("Paper-RS Boost: an extra connection was lost"));
            }
        }
    }

    /** The packet in a frame body: as it is, or inflated when the frame says so. */
    private ByteBuf decompress(final ByteBuf frame, final Inflater inflater) throws DataFormatException {
        if (this.threshold < 0) {
            return frame;
        }
        final int size = readVarInt(frame);
        if (size == 0) {
            return frame;
        }
        final byte[] compressed = new byte[frame.readableBytes()];
        frame.readBytes(compressed);
        inflater.reset();
        inflater.setInput(compressed);
        final byte[] out = new byte[size];
        int done = 0;
        while (done < size && !inflater.finished()) {
            final int n = inflater.inflate(out, done, size - done);
            if (n == 0 && (inflater.needsInput() || inflater.needsDictionary())) {
                throw new DataFormatException("truncated chunk data");
            }
            done += n;
        }
        return Unpooled.wrappedBuffer(out);
    }

    private static byte[] digest(final String label, final byte[] token, final Byte index) {
        try {
            final MessageDigest sha = MessageDigest.getInstance("SHA-256");
            sha.update(label.getBytes(StandardCharsets.US_ASCII));
            sha.update(token);
            if (index != null) {
                sha.update(index);
            }
            return sha.digest();
        } catch (final GeneralSecurityException e) {
            throw new IllegalStateException(e);
        }
    }

    private static int readVarInt(final DataInputStream in) throws IOException {
        int value = 0;
        for (int shift = 0; shift < 35; shift += 7) {
            final int b = in.readUnsignedByte();
            value |= (b & 0x7F) << shift;
            if ((b & 0x80) == 0) {
                return value;
            }
        }
        throw new IOException("varint too long");
    }

    private static int readVarInt(final ByteBuf buf) {
        int value = 0;
        for (int shift = 0; shift < 35; shift += 7) {
            final int b = buf.readUnsignedByte();
            value |= (b & 0x7F) << shift;
            if ((b & 0x80) == 0) {
                return value;
            }
        }
        throw new IllegalArgumentException("varint too long");
    }
}
