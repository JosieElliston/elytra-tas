package jealoustone.etasoracle;

import com.mojang.brigadier.exceptions.CommandSyntaxException;
import jealoustone.elytratas.netlog.NetLog;
import net.fabricmc.fabric.api.client.gametest.v1.FabricClientGameTest;
import net.fabricmc.fabric.api.client.gametest.v1.context.ClientGameTestContext;
import net.fabricmc.fabric.api.client.gametest.v1.context.TestSingleplayerContext;
import net.minecraft.SharedConstants;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.commands.arguments.blocks.BlockStateParser;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.phys.Vec3;

import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;

/**
 * Plays every case in one singleplayer world and writes each to
 * {@code <oracle.out>/<kind>-<seed>/}: the logger's log of the case's ticks
 * ({@code log.tsv.gz}) and the sidecar ({@code oracle.tsv}: world and the state the log
 * lacks). Which cases: {@code ORACLE_CASES=walls:3,air:12}, or {@code ORACLE_KINDS}
 * (default all) times {@code ORACLE_SEEDS} (default {@code 1-8}).
 */
public final class OracleGameTest implements FabricClientGameTest {
    private static final int SETTLE = 20;

    private final List<String> failures = new ArrayList<>();
    private final Map<String, Integer> ticksByKind = new TreeMap<>();

    @Override
    public void runTest(ClientGameTestContext context) {
        Path out = Path.of(System.getProperty("oracle.out", "cases"));
        List<Scenario> cases = cases();
        long t0 = System.nanoTime();
        try (TestSingleplayerContext sp = context.worldBuilder().create()) {
            sp.getConnection().waitForChunksRender();
            context.runOnClient(c -> {
                c.options.renderDistance().set(12);
                c.options.sprintWindow().set(0); // port's SPRINT_WINDOW
            });
            for (String command : List.of("gamemode survival @a",
                    "item replace entity @a armor.chest with elytra[unbreakable={}]",
                    "effect give @a saturation infinite 4 true")) {
                sp.getServer().runCommand(command);
            }
            // No damage, so no fly-into-wall knockback: the server would send its own
            // velocity back to the client a tick later (README.md, "EMC course rules").
            sp.getServer().runOnServer(s -> {
                s.getPlayerList().getPlayers().getFirst().setInvulnerable(true);
                // no random ticks: grass under a placed block would turn to dirt mid-case
                s.getGameRules().set(GameRules.RANDOM_TICK_SPEED, 0, s);
            });
            context.runOnClient(c -> Driver.install(c.player));
            context.waitTicks(5);
            for (Scenario sc : cases) {
                try {
                    runCase(context, sp, sc, out);
                } catch (RuntimeException e) {
                    failures.add(sc.name() + ": " + e);
                    context.runOnClient(c -> Driver.end());
                }
            }
        }
        System.out.printf("=== etas oracle: %d cases in %.0f s, ticks by kind %s%n", cases.size(),
                (System.nanoTime() - t0) / 1e9, ticksByKind);
        if (!failures.isEmpty()) throw new AssertionError(String.join("; ", failures));
    }

    private static List<Scenario> cases() {
        List<Scenario> out = new ArrayList<>();
        String explicit = System.getenv("ORACLE_CASES");
        if (explicit != null && !explicit.isBlank()) {
            for (String c : explicit.split(",")) {
                String[] kv = c.trim().split(":");
                out.add(Scenario.create(kv[0], Long.parseLong(kv[1])));
            }
            return out;
        }
        String kinds = System.getenv().getOrDefault("ORACLE_KINDS", String.join(",", Scenario.KINDS));
        String[] seeds = System.getenv().getOrDefault("ORACLE_SEEDS", "1-8").split("-");
        long lo = Long.parseLong(seeds[0]), hi = Long.parseLong(seeds[seeds.length - 1]);
        for (String kind : kinds.split(",")) {
            for (long s = lo; s <= hi; s++) out.add(Scenario.create(kind.trim(), s));
        }
        return out;
    }

    private void runCase(ClientGameTestContext context, TestSingleplayerContext sp, Scenario sc, Path out) {
        long t0 = System.nanoTime();
        // build, and stand at the start
        sp.getServer().runOnServer(server -> {
            ServerLevel level = server.overworld();
            if (sc.platform != null) level.setBlock(sc.platform, Blocks.GLASS.defaultBlockState(), Block.UPDATE_CLIENTS);
            for (Map.Entry<BlockPos, String> e : sc.blocks.entrySet()) {
                level.setBlock(e.getKey(), parse(e.getValue()), Block.UPDATE_CLIENTS);
            }
        });
        teleport(sp, sc);
        int still = 0;
        for (int i = 0; still < SETTLE; i++) {
            if (i > 600) {
                String st = context.computeOnClient(c -> c.player.position() + " v " + c.player.getDeltaMovement()
                        + " ground " + c.player.onGround() + " glide " + c.player.isFallFlying() + " pose " + c.player.getPose());
                throw new IllegalStateException("never stood still at the start " + sc.start + ": " + st);
            }
            if (i % 100 == 99) teleport(sp, sc);
            context.waitTick();
            boolean ok = context.computeOnClient(c -> {
                LocalPlayer p = c.player;
                return p.onGround() && !p.isFallFlying() && p.getDeltaMovement().horizontalDistanceSqr() == 0.0
                        && Math.abs(p.getX() - sc.start.x) < 1e-9 && Math.abs(p.getZ() - sc.start.z) < 1e-9;
            });
            still = ok ? still + 1 : 0;
        }
        // the client has every placed block (and so their chunks) as the server has it
        for (int i = 0; ; i++) {
            Map<BlockPos, String> now = sp.getServer().computeOnServer(server -> readBack(server.overworld(), sc.blocks.keySet()));
            String diff = context.computeOnClient(c -> {
                int n = 0;
                String first = null;
                for (Map.Entry<BlockPos, String> e : now.entrySet()) {
                    String client = BlockStateParser.serialize(c.level.getBlockState(e.getKey()));
                    if (!client.equals(e.getValue())) {
                        if (n++ == 0) first = e.getKey().toShortString() + " server " + e.getValue() + " client " + client;
                    }
                }
                return n == 0 ? null : n + " differ, e.g. " + first;
            });
            if (diff == null) break;
            if (i > 1200) throw new IllegalStateException("client blocks never matched the server's: " + diff);
            context.waitTick();
        }
        Map<BlockPos, String> atStart = sp.getServer().computeOnServer(server -> readBack(server.overworld(), sc.blocks.keySet()));
        if (sc.platform != null) {
            sp.getServer().runOnServer(s -> s.overworld().setBlock(sc.platform, Blocks.AIR.defaultBlockState(), Block.UPDATE_CLIENTS));
            context.waitFor(c -> c.level.getBlockState(sc.platform).isAir());
        }
        if (!context.computeOnClient(c -> c.player.onGround())) throw new IllegalStateException("fell before the first tick");

        Path[] log = new Path[1];
        context.runOnClient(c -> {
            NetLog.start(c.getConnection().getConnection(), null, c);
            log[0] = NetLog.path();
            Driver.begin(sc);
        });
        if (log[0] == null) throw new IllegalStateException("the logger didn't start");
        String reason = "length";
        for (int i = 0; i < sc.length; i++) {
            context.waitTick();
            String why = context.computeOnClient(c -> Driver.error() != null ? "error: " + Driver.error()
                    : sc.stop(c.player) ? "below minY" : null);
            if (why != null) {
                reason = why;
                break;
            }
        }
        int steps = context.computeOnClient(c -> {
            Driver.end();
            NetLog.stop("oracle case");
            return Driver.steps();
        });
        List<String> records = List.copyOf(Driver.records());
        List<double[]> positions = List.copyOf(Driver.positions());
        if (reason.startsWith("error")) throw new IllegalStateException(reason);

        // the world: every non-air block in a box around the path and the placements
        int[] box = bounds(positions, sc.blocks.keySet());
        Map<BlockPos, String> after = sp.getServer().computeOnServer(server -> readBack(server.overworld(), sc.blocks.keySet()));
        List<String> changed = new ArrayList<>();
        for (BlockPos p : atStart.keySet()) {
            if (!atStart.get(p).equals(after.get(p))) {
                changed.add(String.join("\t", "C", "" + p.getX(), "" + p.getY(), "" + p.getZ(), atStart.get(p), after.get(p)));
            }
        }
        Map<BlockPos, String> world = sp.getServer().computeOnServer(server -> scan(server.overworld(), box, sc.blocks.keySet()));

        Path dir = out.resolve(sc.name());
        try {
            Files.createDirectories(dir);
            Files.move(log[0], dir.resolve("log.tsv.gz"), StandardCopyOption.REPLACE_EXISTING);
            List<String> lines = new ArrayList<>();
            lines.add("#etas-oracle\t1");
            lines.add(String.join("\t", "M", "kind=" + sc.kind, "seed=" + sc.seed, "steps=" + steps, "stop=" + reason.replace(' ', '_'),
                    "origin=" + sc.origin.getX() + "," + sc.origin.getY() + "," + sc.origin.getZ(),
                    "structure=" + (sc instanceof Scenario.Walls w ? w.structure : "-"),
                    "blocks_changed=" + changed.size(),
                    "mc=" + SharedConstants.getCurrentVersion().name(),
                    "java=" + System.getProperty("java.vm.vendor").replace(' ', '_') + "_" + System.getProperty("java.vm.version"),
                    "os=" + System.getProperty("os.name").replace(' ', '_') + "_" + System.getProperty("os.arch")));
            lines.add(String.join("\t", "A", "" + box[0], "" + box[1], "" + box[2], "" + box[3], "" + box[4], "" + box[5]));
            for (Map.Entry<BlockPos, String> e : world.entrySet()) {
                BlockPos p = e.getKey();
                lines.add(String.join("\t", "B", "" + p.getX(), "" + p.getY(), "" + p.getZ(), e.getValue()));
            }
            lines.addAll(changed);
            lines.addAll(records);
            Files.write(dir.resolve("oracle.tsv"), lines);
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
        ticksByKind.merge(sc.kind, steps, Integer::sum);
        System.out.printf("etas oracle: %s (%s): %d ticks, stop %s, %d blocks, %d changed, %.1f s%n", sc.name(),
                sc instanceof Scenario.Walls w ? w.structure : sc.kind, steps, reason, world.size(), changed.size(),
                (System.nanoTime() - t0) / 1e9);
    }

    /**
     * To the start, at rest. why zero the velocity first? the teleport packet carries the
     * server's velocity, and a player still falling from the last case would keep it.
     */
    private static void teleport(TestSingleplayerContext sp, Scenario sc) {
        sp.getServer().runOnServer(server -> {
            ServerPlayer player = server.getPlayerList().getPlayers().getFirst();
            player.setDeltaMovement(Vec3.ZERO);
            player.teleportTo(server.overworld(), sc.start.x, sc.start.y, sc.start.z, Set.of(), sc.startYaw, 0.0F, true);
        });
    }

    private static final Map<String, BlockState> PARSED = new HashMap<>();

    private static BlockState parse(String state) {
        return PARSED.computeIfAbsent(state, s -> {
            try {
                return BlockStateParser.parseForBlock(BuiltInRegistries.BLOCK, s, false).blockState();
            } catch (CommandSyntaxException e) {
                throw new IllegalArgumentException(s, e);
            }
        });
    }

    private static Map<BlockPos, String> readBack(ServerLevel level, Set<BlockPos> at) {
        Map<BlockPos, String> m = new HashMap<>();
        for (BlockPos p : at) m.put(p, BlockStateParser.serialize(level.getBlockState(p)));
        return m;
    }

    /**
     * {@code [x0, y0, z0, x1, y1, z1]}, inclusive: the path's bounds and the placements',
     * 8 blocks wider. why 8? a tick's collision query spans the box plus the move.
     */
    private static int[] bounds(List<double[]> path, Set<BlockPos> placed) {
        double[] lo = {Double.MAX_VALUE, Double.MAX_VALUE, Double.MAX_VALUE};
        double[] hi = {-Double.MAX_VALUE, -Double.MAX_VALUE, -Double.MAX_VALUE};
        for (double[] p : path) {
            for (int i = 0; i < 3; i++) {
                lo[i] = Math.min(lo[i], p[i]);
                hi[i] = Math.max(hi[i], p[i]);
            }
        }
        for (BlockPos b : placed) {
            int[] c = {b.getX(), b.getY(), b.getZ()};
            for (int i = 0; i < 3; i++) {
                lo[i] = Math.min(lo[i], c[i]);
                hi[i] = Math.max(hi[i], c[i]);
            }
        }
        int[] box = new int[6];
        for (int i = 0; i < 3; i++) {
            box[i] = (int) Math.floor(lo[i]) - 8;
            box[i + 3] = (int) Math.floor(hi[i]) + 8;
        }
        // the flat world's ground is at y <= -61: keep it out, the stop rules keep the path above it
        box[1] = Math.max(box[1], -60);
        return box;
    }

    /**
     * Every non-air block in the box: loaded chunks section by section, plus the
     * placements (loading their chunks). A chunk never loaded is still flat-world
     * generation, air above y = -61.
     */
    private static Map<BlockPos, String> scan(ServerLevel level, int[] box, Set<BlockPos> placed) {
        Map<BlockPos, String> m = new TreeMap<>((a, b) -> a.getY() != b.getY() ? Integer.compare(a.getY(), b.getY())
                : a.getZ() != b.getZ() ? Integer.compare(a.getZ(), b.getZ()) : Integer.compare(a.getX(), b.getX()));
        for (BlockPos p : placed) {
            BlockState s = level.getBlockState(p);
            if (!s.isAir()) m.put(p.immutable(), BlockStateParser.serialize(s));
        }
        BlockPos.MutableBlockPos q = new BlockPos.MutableBlockPos();
        for (int cx = box[0] >> 4; cx <= box[3] >> 4; cx++) {
            for (int cz = box[2] >> 4; cz <= box[5] >> 4; cz++) {
                LevelChunk chunk = level.getChunkSource().getChunkNow(cx, cz);
                if (chunk == null) continue;
                for (int sy = box[1] >> 4; sy <= box[4] >> 4; sy++) {
                    int idx = chunk.getSectionIndexFromSectionY(sy);
                    if (idx < 0 || idx >= chunk.getSectionsCount()) continue;
                    LevelChunkSection section = chunk.getSection(idx);
                    if (section.hasOnlyAir()) continue;
                    for (int y = Math.max(sy << 4, box[1]); y <= Math.min((sy << 4) + 15, box[4]); y++)
                        for (int z = Math.max(cz << 4, box[2]); z <= Math.min((cz << 4) + 15, box[5]); z++)
                            for (int x = Math.max(cx << 4, box[0]); x <= Math.min((cx << 4) + 15, box[3]); x++) {
                                BlockState s = section.getBlockState(x & 15, y & 15, z & 15);
                                if (!s.isAir()) m.put(new BlockPos(x, y, z), BlockStateParser.serialize(s));
                            }
                }
            }
        }
        return m;
    }
}
