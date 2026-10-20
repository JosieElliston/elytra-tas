package jealoustone.elytratas.mixin;

import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelHandlerContext;
import jealoustone.elytratas.netlog.NetLog;
import net.minecraft.network.PacketDecoder;
import net.minecraft.network.ProtocolInfo;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.PacketFlow;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import java.util.List;

@Mixin(PacketDecoder.class)
public abstract class PacketDecoderMixin {
    @Shadow
    @Final
    private ProtocolInfo<?> protocolInfo;

    // ByteToMessageDecoder clears `out` before each decode call, and decode adds exactly
    // one packet before returning normally.
    @Inject(method = "decode", at = @At("RETURN"))
    private void elytraTas$decoded(ChannelHandlerContext ctx, ByteBuf input, List<Object> out, CallbackInfo ci) {
        if (protocolInfo.flow() == PacketFlow.CLIENTBOUND && !out.isEmpty() && out.getLast() instanceof Packet<?> packet) {
            NetLog.decoded(ctx.channel(), packet);
        }
    }
}
