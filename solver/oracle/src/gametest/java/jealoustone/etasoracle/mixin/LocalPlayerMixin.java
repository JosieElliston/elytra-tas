package jealoustone.etasoracle.mixin;

import jealoustone.etasoracle.Driver;
import net.minecraft.client.player.LocalPlayer;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** The state physics starts from and ends with, around the whole player tick. */
@Mixin(LocalPlayer.class)
public abstract class LocalPlayerMixin {
    @Inject(method = "tick", at = @At("HEAD"))
    private void oracle$head(CallbackInfo ci) {
        Driver.physics((LocalPlayer) (Object) this, true);
    }

    @Inject(method = "tick", at = @At("RETURN"))
    private void oracle$return(CallbackInfo ci) {
        Driver.physics((LocalPlayer) (Object) this, false);
    }
}
