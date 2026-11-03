package jealoustone.etasoracle.mixin;

import jealoustone.etasoracle.PlaybackGameTest;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.entity.LivingEntity;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Integrated server: while held, the server never ends the player's glide (updateFallFlying is where it does). */
@Mixin(LivingEntity.class)
public abstract class GlideHoldMixin {
    @Inject(method = "updateFallFlying", at = @At("HEAD"), cancellable = true)
    private void oracle$hold(CallbackInfo ci) {
        if (PlaybackGameTest.holdGlide && (Object) this instanceof ServerPlayer) ci.cancel();
    }
}
