package jealoustone.elytratas.mixin;

import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelHandlerContext;
import jealoustone.elytratas.netlog.NetLog;
import net.minecraft.network.PacketEncoder;
import net.minecraft.network.ProtocolInfo;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.PacketFlow;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

@Mixin(PacketEncoder.class)
public abstract class PacketEncoderMixin {
    @Shadow
    @Final
    private ProtocolInfo<?> protocolInfo;

    // MessageToByteEncoder writes the encoded buffer down the pipeline right after this
    // returns, so the next WireTap.write on this thread is this packet's frame.
    @Inject(method = "encode(Lio/netty/channel/ChannelHandlerContext;Lnet/minecraft/network/protocol/Packet;Lio/netty/buffer/ByteBuf;)V",
            at = @At("RETURN"))
    private void elytraTas$encoded(ChannelHandlerContext ctx, Packet<?> packet, ByteBuf output, CallbackInfo ci) {
        if (protocolInfo.flow() == PacketFlow.SERVERBOUND) NetLog.encoded(ctx.channel(), packet);
    }
}
