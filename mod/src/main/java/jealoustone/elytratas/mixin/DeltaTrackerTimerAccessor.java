package jealoustone.elytratas.mixin;

import net.minecraft.client.DeltaTracker;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Accessor;

@Mixin(DeltaTracker.Timer.class)
public interface DeltaTrackerTimerAccessor {
    @Accessor("lastMs")
    long elytraTas$lastMs();

    @Accessor("deltaTickResidual")
    float elytraTas$deltaTickResidual();

    @Accessor("deltaTickResidual")
    void elytraTas$setDeltaTickResidual(float residual);
}
