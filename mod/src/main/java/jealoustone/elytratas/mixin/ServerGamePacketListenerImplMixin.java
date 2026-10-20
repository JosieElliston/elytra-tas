package jealoustone.elytratas.mixin;

import jealoustone.elytratas.netlog.NetLog;
import net.minecraft.network.protocol.game.ServerboundMovePlayerPacket;
import net.minecraft.network.protocol.game.ServerboundPlayerCommandPacket;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.network.ServerGamePacketListenerImpl;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/**
 * Integrated server only. RETURN is reached only on the server thread: on the Netty thread
 * the handlers throw from ensureRunningOnSameThread after queueing the packet.
 */
@Mixin(ServerGamePacketListenerImpl.class)
public abstract class ServerGamePacketListenerImplMixin {
    @Shadow
    public ServerPlayer player;

    @Inject(method = "handleMovePlayer", at = @At("RETURN"))
    private void elytraTas$move(ServerboundMovePlayerPacket packet, CallbackInfo ci) {
        if (NetLog.serverWanted(player)) {
            NetLog.server("move", player, NetLog.name(packet), packet.isOnGround() ? "1" : "0", packet.horizontalCollision() ? "1" : "0");
        }
    }

    @Inject(method = "handlePlayerCommand", at = @At("RETURN"))
    private void elytraTas$command(ServerboundPlayerCommandPacket packet, CallbackInfo ci) {
        if (NetLog.serverWanted(player)) NetLog.server("command", player, packet.getAction().name());
    }
}
