import java.io.PrintWriter;
import java.util.Locale;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.phys.shapes.CollisionContext;

/**
 * Dumps every block state of this Minecraft version as one TSV row:
 * network id, state string, block class, friction, speed factor, jump factor,
 * fluid (-/water/lava), collision boxes (x0,y0,z0,x1,y1,z1;... or -).
 * Shapes use CollisionContext.empty(). Tags aren't bound without a datapack, so
 * anything tag-based (climbable, ...) is left to world.py, by block name.
 */
public class BlockDump {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion();
    Bootstrap.bootStrap();
    try (PrintWriter out = new PrintWriter(new java.util.zip.GZIPOutputStream(new java.io.FileOutputStream(args[0])))) {
      out.println("# " + SharedConstants.getCurrentVersion().name() + " block states: id state class friction speed jump fluid boxes");
      for (BlockState s : Block.BLOCK_STATE_REGISTRY) {
        int id = Block.BLOCK_STATE_REGISTRY.getId(s);
        Block b = s.getBlock();
        StringBuilder boxes = new StringBuilder();
        for (var bb : s.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, CollisionContext.empty()).toAabbs()) {
          if (boxes.length() > 0) boxes.append(';');
          boxes.append(String.format(Locale.ROOT, "%s,%s,%s,%s,%s,%s", bb.minX, bb.minY, bb.minZ, bb.maxX, bb.maxY, bb.maxZ));
        }
        var fs = s.getFluidState();
        var ft = fs.getType();
        String fluid = fs.isEmpty() ? "-"
            : ft == Fluids.WATER || ft == Fluids.FLOWING_WATER ? "water"
            : ft == Fluids.LAVA || ft == Fluids.FLOWING_LAVA ? "lava" : "other";
        out.printf(Locale.ROOT, "%d\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n", id, s.toString().replace("Block{", "").replace("}", ""),
            b.getClass().getSimpleName(), b.getFriction(), b.getSpeedFactor(), b.getJumpFactor(), fluid,
            boxes.length() == 0 ? "-" : boxes);
      }
    }
  }
}
