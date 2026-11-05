package jealoustone.elytratas.mixin;

import jealoustone.elytratas.play.Playback;
import net.minecraft.client.Minecraft;
import net.minecraft.client.multiplayer.ClientPacketListener;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket;
import net.minecraft.network.protocol.game.ClientboundSetEntityMotionPacket;
import net.minecraft.network.protocol.game.ClientboundSystemChatPacket;
import net.minecraft.world.phys.Vec3;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Unique;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/**
 * Playback's events. These handlers first run on the Netty thread and throw from
 * ensureRunningOnSameThread to queue the packet, so HEAD is reached on both threads and
 * RETURN only on the client thread.
 */
@Mixin(ClientPacketListener.class)
public abstract class ClientPacketListenerMixin {
    @Unique
    private boolean elytraTas$wasFallFlying;

    @Inject(method = "handleSetEntityData", at = @At("HEAD"))
    private void elytraTas$entityDataStart(ClientboundSetEntityDataPacket packet, CallbackInfo ci) {
        Minecraft mc = Minecraft.getInstance();
        // client thread only, so the Netty pass can't overwrite the field in between
        if (!mc.isSameThread()) return;
        LocalPlayer p = mc.player;
        elytraTas$wasFallFlying = p != null && packet.id() == p.getId() && p.isFallFlying();
    }

    @Inject(method = "handleSetEntityData", at = @At("RETURN"))
    private void elytraTas$entityData(ClientboundSetEntityDataPacket packet, CallbackInfo ci) {
        LocalPlayer p = Minecraft.getInstance().player;
        if (elytraTas$wasFallFlying && p != null && !p.isFallFlying()) Playback.glideOff();
        elytraTas$wasFallFlying = false;
    }

    @Inject(method = "handleSetEntityMotion", at = @At("RETURN"))
    private void elytraTas$entityMotion(ClientboundSetEntityMotionPacket packet, CallbackInfo ci) {
        LocalPlayer p = Minecraft.getInstance().player;
        if (p == null || packet.id() != p.getId()) return;
        // what lerpMotion set: the decoded payload, which a plan's branch compares against
        Vec3 v = packet.movement();
        Playback.setVelocity(v.x, v.y, v.z);
    }

    @Inject(method = "handleSystemChat", at = @At("RETURN"))
    private void elytraTas$systemChat(ClientboundSystemChatPacket packet, CallbackInfo ci) {
        if (!packet.overlay()) Playback.serverMessage(packet.content().getString());
    }
}
