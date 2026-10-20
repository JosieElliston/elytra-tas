package jealoustone.elytratas.mixin;

import jealoustone.elytratas.netlog.NetLog;
import net.minecraft.server.level.ServerPlayer;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Integrated server only. doTick is the player's physics tick, called from ServerGamePacketListenerImpl.tickPlayer. */
@Mixin(ServerPlayer.class)
public abstract class ServerPlayerMixin {
    @Inject(method = "doTick", at = @At("HEAD"))
    private void elytraTas$tickStart(CallbackInfo ci) {
        ServerPlayer self = (ServerPlayer) (Object) this;
        if (NetLog.serverWanted(self)) NetLog.server("tick_start", self);
    }

    @Inject(method = "doTick", at = @At("RETURN"))
    private void elytraTas$tickEnd(CallbackInfo ci) {
        ServerPlayer self = (ServerPlayer) (Object) this;
        if (NetLog.serverWanted(self)) NetLog.server("tick_end", self);
    }
}
