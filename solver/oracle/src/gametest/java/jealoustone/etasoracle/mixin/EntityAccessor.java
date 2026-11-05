package jealoustone.etasoracle.mixin;

import net.minecraft.network.syncher.EntityDataAccessor;
import net.minecraft.world.entity.Entity;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.gen.Accessor;

@Mixin(Entity.class)
public interface EntityAccessor {
    @Accessor("onGroundNoBlocks")
    boolean oracle$onGroundNoBlocks();

    /** The shared flags (0x80 = fall flying), for the playback test's faked glide-off. */
    @Accessor("DATA_SHARED_FLAGS_ID")
    static EntityDataAccessor<Byte> oracle$sharedFlags() {
        throw new AssertionError();
    }
}
