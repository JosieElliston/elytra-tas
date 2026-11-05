package jealoustone.etasoracle.mixin;

import jealoustone.etasoracle.PlaybackGameTest;
import net.minecraft.core.BlockPos;
import net.minecraft.network.protocol.game.ServerboundMovePlayerPacket;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.network.ServerGamePacketListenerImpl;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/**
 * Integrated server: the playback test's platform goes on the server right before the next
 * move is checked (PlaybackGameTest.removeOnServerMove), so that move never meets it.
 */
@Mixin(ServerGamePacketListenerImpl.class)
public abstract class ServerMoveMixin {
    @Shadow
    public ServerPlayer player;

    @Inject(method = "handleMovePlayer", at = @At("HEAD"))
    private void oracle$removePlatform(ServerboundMovePlayerPacket packet, CallbackInfo ci) {
        BlockPos rm = PlaybackGameTest.removeOnServerMove;
        // HEAD is reached on the Netty thread too, before the packet is queued
        if (rm == null || !(player.level() instanceof ServerLevel level) || !level.getServer().isSameThread()) return;
        PlaybackGameTest.removeOnServerMove = null;
        level.setBlock(rm, Blocks.AIR.defaultBlockState(), Block.UPDATE_CLIENTS);
    }
}
