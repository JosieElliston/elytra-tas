import com.google.gson.JsonElement;
import com.google.gson.JsonParser;
import java.io.InputStreamReader;
import java.io.PrintWriter;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.BedBlock;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.PowderSnowBlock;
import net.minecraft.world.level.block.SlimeBlock;
import net.minecraft.world.level.block.SweetBerryBushBlock;
import net.minecraft.world.level.block.WebBlock;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.CollisionContext;

/**
 * Dumps every block state of this Minecraft version as one TSV row; the columns are
 * listed in the file's first line. world.py's docstring describes them.
 *
 * Collision boxes are CollisionContext.empty() shapes with the position offset taken
 * out; the offset is reconstructed from the offset type and the block's max offsets,
 * and checked here bit for bit against vanilla's getOffset and getCollisionShape at
 * sample positions (the dump aborts on a mismatch).
 *
 * Tags aren't bound without a datapack, so climbable / can_glide_through membership
 * is read from the jar's vanilla tag JSON (data/minecraft/tags/block/), not from
 * BlockState.is(tag). The client uses the server's tags, so world files carry the
 * replay's (world.climb_tags).
 *
 * suffocating is BlockState.isSuffocating(EmptyBlockGetter.INSTANCE, BlockPos.ZERO).
 * Cases that depend on the world aren't covered: shulker boxes (open or closed is
 * block entity state, absent here) and any shape that depends on neighbors.
 *
 * Lines starting with '#' after the column line: "#entity id name" (entity type
 * registry ids), "#offset_check state x z ox oy oz" and "#shape_check state x y z
 * boxes" (vanilla results, doubles as hex bits, for the solver's world tests).
 */
public class BlockDump {
  static final Set<String> CLIMBABLE = new HashSet<>();
  static final Set<String> GLIDE_THROUGH = new HashSet<>();

  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion();
    Bootstrap.bootStrap();
    resolveTag("climbable", CLIMBABLE);
    resolveTag("can_glide_through", GLIDE_THROUGH);
    Method maxH = BlockBehaviour.class.getDeclaredMethod("getMaxHorizontalOffset");
    Method maxV = BlockBehaviour.class.getDeclaredMethod("getMaxVerticalOffset");
    maxH.setAccessible(true);
    maxV.setAccessible(true);
    List<String> checks = new ArrayList<>();
    try (PrintWriter out = new PrintWriter(new java.util.zip.GZIPOutputStream(new java.io.FileOutputStream(args[0])))) {
      out.println("# " + SharedConstants.getCurrentVersion().name()
          + " block states v3: id state class friction speed jump fluid boxes effect offset max_h max_v large block_id default climbable glide_through suffocating");
      for (var t : BuiltInRegistries.ENTITY_TYPE) {
        out.printf(Locale.ROOT, "#entity\t%d\t%s%n", BuiltInRegistries.ENTITY_TYPE.getId(t), BuiltInRegistries.ENTITY_TYPE.getKey(t));
      }
      List<String> rows = new ArrayList<>();
      for (BlockState s : Block.BLOCK_STATE_REGISTRY) {
        int id = Block.BLOCK_STATE_REGISTRY.getId(s);
        Block b = s.getBlock();
        String name = BuiltInRegistries.BLOCK.getKey(b).toString();
        String state = s.toString().replace("Block{", "").replace("}", "");
        float mh = (float) maxH.invoke(b);
        float mv = (float) maxV.invoke(b);
        String offset = offsetType(s, mh, mv);
        Vec3 o0 = s.getOffset(BlockPos.ZERO);
        StringBuilder boxes = new StringBuilder();
        List<double[]> local = new ArrayList<>();
        for (AABB bb : s.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, CollisionContext.empty()).toAabbs()) {
          double[] l = {unoffset(bb.minX, o0.x), unoffset(bb.minY, o0.y), unoffset(bb.minZ, o0.z),
              unoffset(bb.maxX, o0.x), unoffset(bb.maxY, o0.y), unoffset(bb.maxZ, o0.z)};
          local.add(l);
          if (boxes.length() > 0) boxes.append(';');
          boxes.append(String.format(Locale.ROOT, "%s,%s,%s,%s,%s,%s", l[0], l[1], l[2], l[3], l[4], l[5]));
        }
        if (!offset.equals("none")) {
          checkShapes(s, local, mh, mv, offset, state, checks);
        }
        var fs = s.getFluidState();
        var ft = fs.getType();
        String fluid = fs.isEmpty() ? "-"
            : ft == Fluids.WATER || ft == Fluids.FLOWING_WATER ? "water"
            : ft == Fluids.LAVA || ft == Fluids.FLOWING_LAVA ? "lava" : "other";
        boolean climbable = CLIMBABLE.contains(name);
        boolean glideThrough = GLIDE_THROUGH.contains(name);
        // The block's own class, apart from climbing, which comes from the climbable
        // and can_glide_through tags (columns below) because the server sends its own
        // tags: EMC's lack can_glide_through (2026-10-19 replay), so vines end a glide there.
        String effect = fluid.equals("lava") ? "lava"
            : fluid.equals("water") ? "water"
            : b instanceof WebBlock || b instanceof SweetBerryBushBlock || b instanceof PowderSnowBlock ? "stuck"
            : b instanceof SlimeBlock || b instanceof BedBlock ? "bounce"
            : "none";
        rows.add(String.format(Locale.ROOT, "%d\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%d\t%d\t%d\t%d\t%d\t%d",
            id, state, b.getClass().getSimpleName(), b.getFriction(), b.getSpeedFactor(), b.getJumpFactor(), fluid,
            boxes.length() == 0 ? "-" : boxes, effect, offset, mh, mv, s.hasLargeCollisionShape() ? 1 : 0,
            BuiltInRegistries.BLOCK.getId(b), s == b.defaultBlockState() ? 1 : 0, climbable ? 1 : 0, glideThrough ? 1 : 0,
            s.isSuffocating(EmptyBlockGetter.INSTANCE, BlockPos.ZERO) ? 1 : 0));
      }
      for (String c : checks) out.println(c);
      for (String r : rows) out.println(r);
    }
  }

  static void resolveTag(String tag, Set<String> into) throws Exception {
    String path = "data/minecraft/tags/block/" + tag + ".json";
    var in = BlockDump.class.getClassLoader().getResourceAsStream(path);
    if (in == null) throw new IllegalStateException("no " + path + " in the jar");
    JsonElement root = JsonParser.parseReader(new InputStreamReader(in, StandardCharsets.UTF_8));
    for (JsonElement v : root.getAsJsonObject().getAsJsonArray("values")) {
      String id = v.isJsonObject() ? v.getAsJsonObject().get("id").getAsString() : v.getAsString();
      if (id.startsWith("#")) {
        resolveTag(id.substring(1).replace("minecraft:", ""), into);
      } else {
        into.add(id);
      }
    }
  }

  /** l with l + o == c exactly, so that vanilla's (l + o) + pos is reproducible. */
  static double unoffset(double c, double o) {
    double l = c - o;
    if (l + o != c) throw new IllegalStateException("can't un-offset " + c + " by " + o);
    return l;
  }

  static String offsetType(BlockState s, float mh, float mv) {
    if (!s.hasOffsetFunction()) return "none";
    for (int x = -8; x < 8; x++) {
      for (int z = -8; z < 8; z++) {
        if (s.getOffset(new BlockPos(x, 0, z)).y != 0.0) return "xyz";
      }
    }
    return "xz";
  }

  /** BlockBehaviour.Properties.offsetType's functions, transcribed. */
  static Vec3 offset(BlockPos pos, float mh, float mv, String type) {
    long seed = net.minecraft.util.Mth.getSeed(pos.getX(), 0, pos.getZ());
    double x = net.minecraft.util.Mth.clamp(((float) (seed & 15L) / 15.0F - 0.5) * 0.5, -mh, mh);
    double z = net.minecraft.util.Mth.clamp(((float) (seed >> 8 & 15L) / 15.0F - 0.5) * 0.5, -mh, mh);
    double y = type.equals("xyz") ? ((float) (seed >> 4 & 15L) / 15.0F - 1.0) * mv : 0.0;
    return new Vec3(x, y, z);
  }

  static final int[][] SAMPLES = {{0, 0, 0}, {1, 64, 0}, {0, -60, 1}, {-1, 166, -1}, {3, 165, 56}, {-17, 100, 33},
      {29999983, 0, -29999983}, {123456, 70, -654321}, {-5, 12, 9}, {15, 0, 15}, {16, 1, -16}, {-1000, 200, 777}};

  static void checkShapes(BlockState s, List<double[]> local, float mh, float mv, String type, String state, List<String> checks) {
    var rnd = new java.util.Random(state.hashCode());
    boolean emit = s == s.getBlock().defaultBlockState() && !local.isEmpty();
    for (int i = 0; i < SAMPLES.length + 200; i++) {
      BlockPos pos = i < SAMPLES.length ? new BlockPos(SAMPLES[i][0], SAMPLES[i][1], SAMPLES[i][2])
          : new BlockPos(rnd.nextInt(2000001) - 1000000, rnd.nextInt(384) - 64, rnd.nextInt(2000001) - 1000000);
      Vec3 o = s.getOffset(pos);
      Vec3 mine = offset(pos, mh, mv, type);
      if (!o.equals(mine)) throw new IllegalStateException(state + " offset at " + pos + ": " + o + " vs " + mine);
      List<AABB> got = s.getCollisionShape(EmptyBlockGetter.INSTANCE, pos, CollisionContext.empty()).move(pos).toAabbs();
      if (got.size() != local.size()) throw new IllegalStateException(state + " box count at " + pos);
      StringBuilder hex = new StringBuilder();
      for (int k = 0; k < got.size(); k++) {
        AABB g = got.get(k);
        double[] l = local.get(k);
        double[] want = {(l[0] + o.x) + pos.getX(), (l[1] + o.y) + pos.getY(), (l[2] + o.z) + pos.getZ(),
            (l[3] + o.x) + pos.getX(), (l[4] + o.y) + pos.getY(), (l[5] + o.z) + pos.getZ()};
        double[] have = {g.minX, g.minY, g.minZ, g.maxX, g.maxY, g.maxZ};
        for (int a = 0; a < 6; a++) {
          if (Double.doubleToRawLongBits(want[a]) != Double.doubleToRawLongBits(have[a])) {
            throw new IllegalStateException(state + " box at " + pos + ": " + have[a] + " vs " + want[a]);
          }
          if (hex.length() > 0) hex.append(k > 0 && a == 0 ? ';' : ',');
          hex.append(Long.toHexString(Double.doubleToRawLongBits(have[a])));
        }
      }
      if (emit && i < SAMPLES.length) {
        checks.add(String.format(Locale.ROOT, "#offset_check\t%s\t%d\t%d\t%s\t%s\t%s", state, pos.getX(), pos.getZ(),
            Long.toHexString(Double.doubleToRawLongBits(o.x)), Long.toHexString(Double.doubleToRawLongBits(o.y)),
            Long.toHexString(Double.doubleToRawLongBits(o.z))));
        checks.add(String.format(Locale.ROOT, "#shape_check\t%s\t%d\t%d\t%d\t%s", state, pos.getX(), pos.getY(), pos.getZ(), hex));
      }
    }
  }
}
