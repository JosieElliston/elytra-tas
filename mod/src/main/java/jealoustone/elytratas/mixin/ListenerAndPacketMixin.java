package jealoustone.elytratas.mixin;

import jealoustone.elytratas.netlog.NetLog;
import jealoustone.elytratas.phase.PhaseTracker;
import net.minecraft.network.PacketListener;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.PacketFlow;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** A packet that a handler queued from the Netty thread, run by PacketProcessor.processQueuedPackets. */
@Mixin(targets = "net.minecraft.network.PacketProcessor$ListenerAndPacket")
public abstract class ListenerAndPacketMixin {
    @Shadow
    @Final
    private PacketListener listener;

    @Shadow
    @Final
    private Packet<?> packet;

    @Inject(method = "handle", at = @At("HEAD"))
    private void elytraTas$handle(CallbackInfo ci) {
        if (listener.flow() != PacketFlow.CLIENTBOUND) return;
        NetLog.mainHandle(packet);
        PhaseTracker.mainHandle(listener, packet);
    }
}
