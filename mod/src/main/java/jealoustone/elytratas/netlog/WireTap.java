package jealoustone.elytratas.netlog;

import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import jealoustone.elytratas.phase.PhaseTracker;

/**
 * First handler in a client connection's pipeline, next to the socket. Outbound, it sees
 * each packet's final frame (encoded, compressed, length-prefixed, encrypted) and the
 * flush that writes it to the socket; inbound, each raw read before frame splitting.
 * It only timestamps and passes messages through unchanged. Also feeds {@link PhaseTracker}.
 */
public final class WireTap extends ChannelDuplexHandler {
    public static final String NAME = "elytra_tas_wire";

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof ByteBuf buf) NetLog.wireRead(ctx.channel(), buf.readableBytes());
        ctx.fireChannelRead(msg);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof ByteBuf buf) {
            NetLog.wireWrite(ctx.channel(), buf.readableBytes());
            PhaseTracker.wireWrite(ctx.channel());
        }
        ctx.write(msg, promise);
    }

    @Override
    public void flush(ChannelHandlerContext ctx) throws Exception {
        long start = System.nanoTime();
        // HeadContext.flush writes to the socket synchronously, unless its send buffer is full.
        ctx.flush();
        NetLog.wireFlush(ctx.channel(), start);
    }
}
