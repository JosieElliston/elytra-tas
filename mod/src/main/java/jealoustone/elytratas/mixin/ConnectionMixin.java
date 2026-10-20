package jealoustone.elytratas.mixin;

import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPipeline;
import jealoustone.elytratas.netlog.NetLog;
import jealoustone.elytratas.netlog.WireTap;
import net.minecraft.network.Connection;
import net.minecraft.network.DisconnectionDetails;
import net.minecraft.network.PacketListener;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.PacketFlow;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

@Mixin(Connection.class)
public abstract class ConnectionMixin {
    @Shadow
    public abstract PacketFlow getReceiving();

    @Shadow
    public abstract boolean isConnected();

    // Runs from the channel initializer for every client connection (including server-list
    // pings); NetLog ignores channels other than the logged session's.
    @Inject(method = "configurePacketHandler", at = @At("HEAD"))
    private void elytraTas$addWireTap(ChannelPipeline pipeline, CallbackInfo ci) {
        if (getReceiving() == PacketFlow.CLIENTBOUND) pipeline.addFirst(WireTap.NAME, new WireTap());
    }

    // send(Packet) and send(Packet, listener) both delegate here.
    @Inject(method = "send(Lnet/minecraft/network/protocol/Packet;Lio/netty/channel/ChannelFutureListener;Z)V", at = @At("HEAD"))
    private void elytraTas$send(Packet<?> packet, ChannelFutureListener listener, boolean flush, CallbackInfo ci) {
        NetLog.sent((Connection) (Object) this, packet);
    }

    @Inject(method = "genericsFtw", at = @At("HEAD"))
    private static void elytraTas$handleStart(Packet<?> packet, PacketListener listener, CallbackInfo ci) {
        NetLog.inlineHandleStart(packet);
    }

    // Not reached when the handler queued the packet for the client thread: it throws
    // RunningOnDifferentThreadException instead of returning.
    @Inject(method = "genericsFtw", at = @At("RETURN"))
    private static void elytraTas$handledInline(Packet<?> packet, PacketListener listener, CallbackInfo ci) {
        NetLog.inlineHandled(packet);
    }

    // A client-side quit closes the channel through disconnect() and drops the Connection
    // without ever calling handleDisconnection, so stop on all three paths.
    @Inject(method = "disconnect(Lnet/minecraft/network/DisconnectionDetails;)V", at = @At("RETURN"))
    private void elytraTas$disconnect(DisconnectionDetails details, CallbackInfo ci) {
        if ((Object) this == NetLog.connection()) NetLog.stop("disconnect");
    }

    @Inject(method = "channelInactive", at = @At("HEAD"))
    private void elytraTas$channelInactive(ChannelHandlerContext ctx, CallbackInfo ci) {
        if ((Object) this == NetLog.connection()) NetLog.stop("channel_inactive");
    }

    @Inject(method = "handleDisconnection", at = @At("RETURN"))
    private void elytraTas$disconnected(CallbackInfo ci) {
        if ((Object) this == NetLog.connection() && !isConnected()) NetLog.stop("disconnect");
    }
}
