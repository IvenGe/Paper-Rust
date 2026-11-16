package io.papermc.paperrs.boost;

import io.netty.buffer.Unpooled;
import java.util.concurrent.CompletableFuture;
import net.fabricmc.api.ClientModInitializer;
import net.fabricmc.fabric.api.client.networking.v1.ClientLoginNetworking;
import net.fabricmc.fabric.api.client.networking.v1.ClientPlayConnectionEvents;
import net.fabricmc.fabric.api.client.networking.v1.ClientPlayNetworking;
import net.fabricmc.fabric.api.networking.v1.PayloadTypeRegistry;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.resources.Identifier;

/**
 * Paper-RS Boost, the client side. A Paper-RS server that allows it asks during login (a login
 * query on {@code paper-rs:boost}); the mod answers how many extra connections it opens, opens
 * them once in the game, and the server sends the chunks over those while markers on the main
 * connection keep their order. Other servers never ask, and then the mod does nothing.
 */
public final class BoostClient implements ClientModInitializer {
    public static final Identifier CHANNEL = Identifier.fromNamespaceAndPath("paper-rs", "boost");
    private static final int VERSION = 1;

    /** The current connection's session, if the server asked. */
    private static volatile Session session;

    @Override
    public void onInitializeClient() {
        PayloadTypeRegistry.clientboundPlay().register(ChunkMarker.TYPE, ChunkMarker.CODEC);

        ClientLoginNetworking.registerGlobalReceiver(CHANNEL, (client, handler, buf, callbacks) -> {
            final int version = buf.readVarInt();
            final int offered = buf.readVarInt();
            final int threshold = buf.readVarInt();
            final byte[] token = new byte[16];
            buf.readBytes(token);
            if (version != VERSION || offered <= 0) {
                return CompletableFuture.completedFuture(null);
            }
            // As many as the server offers, up to one per two cores.
            final int streams = Math.max(1, Math.min(offered, Runtime.getRuntime().availableProcessors() / 2));
            closeSession();
            session = new Session(token, streams, threshold);
            final FriendlyByteBuf answer = new FriendlyByteBuf(Unpooled.buffer());
            answer.writeVarInt(VERSION);
            answer.writeVarInt(streams);
            return CompletableFuture.completedFuture(answer);
        });

        ClientPlayConnectionEvents.JOIN.register((listener, sender, client) -> {
            final Session s = session;
            if (s != null) {
                s.open(listener);
            }
        });
        ClientPlayConnectionEvents.DISCONNECT.register((listener, client) -> closeSession());

        ClientPlayNetworking.registerGlobalReceiver(ChunkMarker.TYPE, (marker, context) -> {
            final Session s = session;
            if (s != null && context.client().getConnection() != null) {
                s.marker(marker.seq(), context.client().getConnection());
            }
        });
    }

    private static void closeSession() {
        final Session old = session;
        session = null;
        if (old != null) {
            old.close();
        }
    }
}
