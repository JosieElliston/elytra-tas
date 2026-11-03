package jealoustone.etasoracle.mixin;

import net.minecraft.client.player.LocalPlayer;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Accessor;

@Mixin(LocalPlayer.class)
public interface LocalPlayerAccessor {
    @Accessor("sprintTriggerTime")
    int oracle$sprintTriggerTime();

    @Accessor("autoJumpTime")
    int oracle$autoJumpTime();

    @Accessor("autoJumpTime")
    void oracle$setAutoJumpTime(int ticks);

    @Accessor("crouching")
    boolean oracle$crouching();
}
