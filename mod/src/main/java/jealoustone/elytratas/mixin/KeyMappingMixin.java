package jealoustone.elytratas.mixin;

import jealoustone.elytratas.netlog.NetLog;
import net.minecraft.client.KeyMapping;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Logs key mapping state changes. setDown is where KeyMapping.set, release and the toggle subclass all end up. */
@Mixin(KeyMapping.class)
public abstract class KeyMappingMixin {
    @Shadow
    private boolean isDown;

    @Shadow
    public abstract String getName();

    @Inject(method = "setDown", at = @At("HEAD"))
    private void elytraTas$setDown(boolean down, CallbackInfo ci) {
        if (down != isDown) NetLog.key(getName(), down);
    }
}
