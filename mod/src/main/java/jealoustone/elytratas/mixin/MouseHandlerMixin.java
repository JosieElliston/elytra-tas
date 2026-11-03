package jealoustone.elytratas.mixin;

import jealoustone.elytratas.play.Playback;
import net.minecraft.client.MouseHandler;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** While a plan plays, the mouse doesn't turn the player. why? physics must only see plan rotations. */
@Mixin(MouseHandler.class)
public abstract class MouseHandlerMixin {
    @Inject(method = "turnPlayer", at = @At("HEAD"), cancellable = true)
    private void elytraTas$turnPlayer(double mousea, CallbackInfo ci) {
        if (Playback.blocksMouse()) ci.cancel();
    }
}
