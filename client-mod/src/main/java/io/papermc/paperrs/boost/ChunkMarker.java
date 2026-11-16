package io.papermc.paperrs.boost;

import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.resources.Identifier;

/**
 * The server's marker on the main connection ({@code paper-rs:boost/chunk}): the chunk with
 * this number, which comes over one of the extra connections, belongs here among the rest of
 * what the server sends.
 */
public record ChunkMarker(int seq) implements CustomPacketPayload {
    public static final Type<ChunkMarker> TYPE = new Type<>(Identifier.fromNamespaceAndPath("paper-rs", "boost/chunk"));
    public static final StreamCodec<RegistryFriendlyByteBuf, ChunkMarker> CODEC =
        StreamCodec.of((buf, marker) -> buf.writeVarInt(marker.seq()), buf -> new ChunkMarker(buf.readVarInt()));

    @Override
    public Type<? extends CustomPacketPayload> type() {
        return TYPE;
    }
}
