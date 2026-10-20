package jealoustone.elytratas.mixin;

import jealoustone.elytratas.netlog.NetLog;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.entity.LivingEntity;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Unique;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Integrated server only: the two places the server ends a glide. */
@Mixin(LivingEntity.class)
public abstract class LivingEntityMixin {
    @Unique
    private boolean elytraTas$wasFallFlying;

    @Inject(method = "updateFallFlying", at = @At("HEAD"))
    private void elytraTas$updateFallFlyingStart(CallbackInfo ci) {
        elytraTas$wasFallFlying = ((LivingEntity) (Object) this).isFallFlying();
    }

    @Inject(method = "updateFallFlying", at = @At("RETURN"))
    private void elytraTas$updateFallFlyingEnd(CallbackInfo ci) {
        if ((Object) this instanceof ServerPlayer player && elytraTas$wasFallFlying && !player.isFallFlying()
                && NetLog.serverWanted(player)) {
            NetLog.server("glide_end", player, "updateFallFlying");
        }
    }

    @Inject(method = "stopFallFlying", at = @At("HEAD"))
    private void elytraTas$stopFallFlying(CallbackInfo ci) {
        if ((Object) this instanceof ServerPlayer player && NetLog.serverWanted(player)) {
            NetLog.server("glide_end", player, "stopFallFlying");
        }
    }
}
