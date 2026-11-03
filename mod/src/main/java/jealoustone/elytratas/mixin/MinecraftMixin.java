package jealoustone.elytratas.mixin;

import jealoustone.elytratas.play.Playback;
import net.minecraft.client.Minecraft;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/**
 * renderFrame runs after the frame's ticks and mouse turn and before camera setup, so a
 * rotation set here shows in this frame. Fabric's render events all fire after camera setup.
 */
@Mixin(Minecraft.class)
public abstract class MinecraftMixin {
    @Inject(method = "renderFrame", at = @At("HEAD"))
    private void elytraTas$renderFrame(boolean advanceGameTime, CallbackInfo ci) {
        Playback.frame((Minecraft) (Object) this);
    }
}
