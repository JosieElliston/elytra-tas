package jealoustone.etasoracle;

import jealoustone.elytratas.netlog.NetLog;
import jealoustone.elytratas.play.Plan;
import jealoustone.elytratas.play.PlanQueue;
import jealoustone.elytratas.play.Playback;
import jealoustone.etasoracle.mixin.EntityAccessor;
import jealoustone.etasoracle.mixin.LocalPlayerAccessor;
import jealoustone.etasoracle.mixin.MinecraftAccessor;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientTickEvents;
import net.fabricmc.fabric.api.client.gametest.v1.FabricClientGameTest;
import net.fabricmc.fabric.api.client.gametest.v1.context.ClientGameTestContext;
import net.fabricmc.fabric.api.client.gametest.v1.context.TestSingleplayerContext;
import net.fabricmc.fabric.api.event.player.UseItemCallback;
import com.mojang.blaze3d.platform.FramerateLimitTracker;
import net.minecraft.SharedConstants;
import net.minecraft.client.InactivityFpsLimit;
import net.minecraft.client.Minecraft;
import net.minecraft.client.User;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.commands.arguments.blocks.BlockStateParser;
import net.minecraft.core.BlockPos;
import net.minecraft.network.chat.Component;
import net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket;
import net.minecraft.network.protocol.game.ClientboundSetEntityMotionPacket;
import net.minecraft.network.syncher.SynchedEntityData;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.InteractionResult;
import net.minecraft.util.Util;
import net.minecraft.world.entity.decoration.ArmorStand;
import net.minecraft.world.entity.player.Input;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.phys.Vec3;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
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
import java.util.function.Consumer;
import java.util.zip.GZIPInputStream;

/**
 * The elytra-tas mod's {@code /etas play} in a singleplayer world (README.md "Playback
 * tests"). Which: {@code PLAYBACK_TESTS=a,b,c,d,e,f} (default all, {@code none} skips).
 *
 * <ul>
 * <li>(a) a plan made from an oracle case ({@code PLAYBACK_CASE}, default air-1, read from
 * {@code oracle.out}), expected poses from the case's own log, flown twice: no abort.</li>
 * <li>(b) ebounces: a run flown by the Driver with the server's glide-off held (nothing
 * ends the glide, which is what port models) gives the plan, with {@code @react glide_off
 * press_jump} after each landing; playback then flies it with the vanilla glide-off and
 * must match it. The held run is also written as an oracle case for port's replay.</li>
 * <li>(c) a divergence injected mid-plan aborts on the right tick with the keys released.</li>
 * <li>(d) branches (format 2): the client is handed velocities and glide-offs at chosen
 * lines (the real handlers, called between ticks); each branch, a rejoin, an
 * {@code @ignore}, a payload within {@code velocity_tol}, and the aborts for an unlisted
 * observation and a missing one. Expected poses come from Driver runs with the same
 * injections.</li>
 * </ul>
 * The slot-1 reset item is a stick here: using it teleports to the current {@link Target}
 * on the server, standing in for EMC's checkpoint item. "Course complete!" is sent by the
 * test near each plan's end. Account checks cover both singleplayer accounts;
 * flight cases run as {@link Playback#ALT}.
 */
public final class PlaybackGameTest implements FabricClientGameTest {
    /** Read on the server thread (GlideHoldMixin): keep the server from ending the glide. */
    public static volatile boolean holdGlide;

    private record Target(Vec3 pos, float yaw, BlockPos platform) {
    }

    private static volatile Target target;
    /** Removed on both sides on the tick line 1 is applied, as the oracle removes its platform before the first tick. */
    private static volatile BlockPos removeOnLine1;
    /** Read on the server thread (ServerMoveMixin): removed there before the next move is handled. */
    public static volatile BlockPos removeOnServerMove;
    /** (c): shift the player's x by {@link #nudge} on the tick this line is applied, in this attempt. */
    private static volatile int nudgeLine = -1, nudgeAttempt = -1;
    private static volatile double nudge;
    private static volatile String nudged;
    /** (c): the playback input's state on the tick of an abort, after the handler released it. */
    private static volatile String atAbort;
    private static int lastHistory;
    /** The Driver's plain input, which playback swaps out and must put back. */
    private static volatile net.minecraft.client.player.ClientInput driverInput;
    private static boolean registered;

    /** (d): hand the client {@code kind} ("velocity", or "glide_off") before line {@code line}; {@code v} null = its own velocity. */
    private record Inject(int line, String kind, double[] v) {
    }

    private static volatile List<Inject> injects = List.of();
    /** (d): while a Driver run records, injections key on its steps instead of playback's lines. */
    private static volatile boolean recording;
    private static final List<String> injected = new java.util.concurrent.CopyOnWriteArrayList<>();

    private static final int FINISH_BEFORE_END = 25;

    private final List<String> failures = new ArrayList<>();
    private final List<String> summary = new ArrayList<>();

    @Override
    public void runTest(ClientGameTestContext context) {
        String which = System.getenv().getOrDefault("PLAYBACK_TESTS", "a,b,c,d,e,f");
        if (which.equals("none")) return;
        Set<String> tests = Set.of(which.split(","));
        Path out = Path.of(System.getProperty("playback.out", "build/playback"));
        Path cases = Path.of(System.getProperty("oracle.out", "cases"));
        register();
        User before = context.computeOnClient(Minecraft::getUser);
        try (TestSingleplayerContext sp = context.worldBuilder().create()) {
            sp.getConnection().waitForChunksRender();
            context.runOnClient(c -> {
                c.options.renderDistance().set(12);
                c.options.sprintWindow().set(0); // port's SPRINT_WINDOW; Playback refuses others
            });
            for (String command : List.of("gamemode survival @a",
                    "item replace entity @a armor.chest with elytra[unbreakable={}]",
                    "item replace entity @a hotbar.0 with stick",
                    "effect give @a saturation infinite 4 true")) {
                sp.getServer().runCommand(command);
            }
            sp.getServer().runOnServer(s -> {
                s.getPlayerList().getPlayers().getFirst().setInvulnerable(true);
                s.getGameRules().set(GameRules.RANDOM_TICK_SPEED, 0, s);
            });
            context.runOnClient(c -> {
                Driver.install(c.player);
                driverInput = c.player.input;
            });
            context.waitTicks(5);

            check(Playback.accountAllowed("deftenvy", false), "alt allowed in multiplayer");
            check(Playback.accountAllowed("deftenvy", true), "alt allowed in singleplayer");
            check(!Playback.accountAllowed("lqmbda", false), "main refused in multiplayer");
            check(Playback.accountAllowed("lqmbda", true), "main allowed in singleplayer");
            check(!Playback.accountAllowed("other", false), "other account refused in multiplayer");
            check(!Playback.accountAllowed("other", true), "other account refused in singleplayer");
            refusals(context, out, false);
            context.runOnClient(c -> ((MinecraftAccessor) c).oracle$setUser(new User("lqmbda", before.getProfileId(),
                    before.getAccessToken(), before.getXuid(), before.getClientId())));
            refusals(context, out, true);
            context.runOnClient(c -> ((MinecraftAccessor) c).oracle$setUser(new User(Playback.ALT, before.getProfileId(),
                    before.getAccessToken(), before.getXuid(), before.getClientId())));
            refusals(context, out, true);

            if (tests.contains("a") || tests.contains("c") || tests.contains("e") || tests.contains("f")) {
                String name = System.getenv().getOrDefault("PLAYBACK_CASE", "air-1");
                OracleCase oc = OracleCase.read(cases.resolve(name));
                if (oc == null) {
                    failures.add("(a)/(c): no oracle case " + name + " in " + cases + " (run the oracle first)");
                } else {
                    prepareOracleWorld(context, sp, oc);
                    if (tests.contains("a")) caseA(context, sp, oc, out);
                    if (tests.contains("c")) caseC(context, sp, oc, out);
                    if (tests.contains("e")) caseE(context, oc);
                    if (tests.contains("f")) caseF(context, sp, oc);
                }
            }
            if (tests.contains("b")) caseB(context, sp, out);
            if (tests.contains("d")) {
                caseDVelocity(context, sp, out);
                caseDGlide(context, sp, out);
            }
        } catch (RuntimeException | AssertionError e) {
            failures.add("error: " + e);
            e.printStackTrace();
        } finally {
            context.runOnClient(c -> {
                PlanQueue.disable("test end");
                Playback.stop("test end");
                ((MinecraftAccessor) c).oracle$setUser(before);
            });
        }
        System.out.println("=== etas playback:\n  " + String.join("\n  ", summary));
        if (!failures.isEmpty()) throw new AssertionError(String.join("; ", failures));
    }

    private void check(boolean ok, String what) {
        if (!ok) failures.add(what);
        System.out.println("etas playback: " + (ok ? "ok   " : "FAIL ") + what);
    }

    // --- hooks ---------------------------------------------------------------------------

    private static void register() {
        if (registered) return;
        registered = true;
        // the reset item, server side
        UseItemCallback.EVENT.register((player, level, hand) -> {
            Target t = target;
            if (!(level instanceof ServerLevel sl) || !(player instanceof ServerPlayer sp) || t == null
                    || hand != InteractionHand.MAIN_HAND || !player.getMainHandItem().is(Items.STICK)) {
                return InteractionResult.PASS;
            }
            if (t.platform != null) sl.setBlock(t.platform, Blocks.GLASS.defaultBlockState(), Block.UPDATE_CLIENTS);
            sp.setDeltaMovement(Vec3.ZERO);
            sp.teleportTo(sl, t.pos.x, t.pos.y, t.pos.z, Set.of(), t.yaw, 0.0F, true);
            return InteractionResult.SUCCESS;
        });
        // (d): after a tick's physics, so a packet handled here is seen before the next line
        ClientTickEvents.END_CLIENT_TICK.register(c -> {
            List<Inject> js = injects;
            if (c.player == null || js.isEmpty()) return;
            int line;
            if (recording) {
                line = Driver.steps();
            } else {
                Playback.Status s = Playback.status();
                if (s.phase() != Playback.Phase.PLAY) return;
                line = s.line();
            }
            for (Inject j : js) {
                if (j.line() != line + 1) continue;
                LocalPlayer p = c.player;
                if (j.kind().equals("velocity")) {
                    Vec3 v = j.v() == null ? p.getDeltaMovement() : new Vec3(j.v()[0], j.v()[1], j.v()[2]);
                    c.getConnection().handleSetEntityMotion(new ClientboundSetEntityMotionPacket(p.getId(), v));
                    injected.add("velocity " + v + " before line " + j.line());
                } else {
                    byte f = p.getEntityData().get(EntityAccessor.oracle$sharedFlags());
                    c.getConnection().handleSetEntityData(new ClientboundSetEntityDataPacket(p.getId(),
                            List.of(SynchedEntityData.DataValue.create(EntityAccessor.oracle$sharedFlags(), (byte) (f & ~0x80)))));
                    injected.add("glide_off (was gliding " + ((f & 0x80) != 0) + ", now " + p.isFallFlying() + ") before line " + j.line());
                }
            }
        });
        // registered after the mod's, so it runs after Playback.step in the same tick, before physics
        ClientTickEvents.START_LEVEL_TICK.register(level -> {
            Minecraft c = Minecraft.getInstance();
            if (c.player == null || Playback.status().plan() == null) return;
            Playback.Status s = Playback.status();
            if (s.history().size() != lastHistory) {
                lastHistory = s.history().size();
                if (lastHistory == 0) return; // a new /etas play or queue plan cleared it
                Playback.Attempt a = s.history().getLast();
                if (a.abortLine() >= 0) {
                    // the abort happened in this tick's handler, right before this, and this
                    // tick's physics runs with what is here
                    Input k = c.player.input.keyPresses;
                    atAbort = "attempt " + a.n() + " line " + a.abortLine() + ": keys " + Driver.bits(k)
                            + " move " + c.player.input.getMoveVector().x + "," + c.player.input.getMoveVector().y + " restored " + (c.player.input == driverInput)
                            + " autoJump " + ((LocalPlayerAccessor) c.player).oracle$autoJumpTime();
                }
            }
            if (s.phase() != Playback.Phase.PLAY) return;
            BlockPos rm = removeOnLine1;
            if (rm != null && s.line() == 1) {
                c.level.setBlock(rm, Blocks.AIR.defaultBlockState(), Block.UPDATE_ALL);
                // and on the server right before it checks this tick's move (ServerMoveMixin): queued
                // with execute, the removal sometimes ran after that move, and the server sent the
                // player back (2026-11-03 10:46: both attempts of (c))
                removeOnServerMove = rm;
            }
            if (s.line() == nudgeLine && s.attempt() == nudgeAttempt) {
                LocalPlayer p = c.player;
                p.setPos(p.getX() + nudge, p.getY(), p.getZ());
                nudged = "attempt " + s.attempt() + " line " + s.line() + " by " + nudge;
            }
        });
    }

    /** Runs the command, then ticks until playback stops, sending "Course complete!" near each attempt's end. */
    private Playback.Status play(ClientGameTestContext context, TestSingleplayerContext sp, String command, int lines,
                                 int timeout, Consumer<Playback.Status> perTick) {
        context.runOnClient(c -> {
            lastHistory = 0;
            c.player.connection.sendCommand(command);
        });
        context.waitTick();
        if (!context.computeOnClient(c -> Playback.active())) {
            throw new IllegalStateException("/" + command + " didn't start (see the chat log)");
        }
        int sentFor = -1;
        for (int i = 0; ; i++) {
            if (i > timeout) {
                context.runOnClient(c -> Playback.stop("test timeout"));
                throw new IllegalStateException("/" + command + ": timed out");
            }
            context.waitTick();
            Playback.Status s = context.computeOnClient(c -> Playback.status());
            if (perTick != null) perTick.accept(s);
            if (s.phase() == Playback.Phase.IDLE) return s;
            if (s.phase() == Playback.Phase.PLAY && s.line() >= lines - FINISH_BEFORE_END && s.attempt() != sentFor) {
                sentFor = s.attempt();
                sp.getServer().runOnServer(server -> server.getPlayerList().getPlayers().getFirst()
                        .sendSystemMessage(Component.literal("Course complete! Your time: 1.000 seconds (test)")));
            }
        }
    }

    private static Path startLog(ClientGameTestContext context) {
        return context.computeOnClient(c -> {
            NetLog.start(c.getConnection().getConnection(), null, c);
            return NetLog.path();
        });
    }

    private static Path keepLog(ClientGameTestContext context, Path log, Path to) {
        context.runOnClient(c -> NetLog.stop("playback test"));
        try {
            Files.createDirectories(to.getParent());
            Files.move(log, to, StandardCopyOption.REPLACE_EXISTING);
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
        return to;
    }

    // --- refusals ------------------------------------------------------------------------

    private void refusals(ClientGameTestContext context, Path out, boolean allowed) {
        String user = context.computeOnClient(c -> c.getUser().getName());
        String start = "start=" + hex(0.5) + " " + hex(-60.0) + " " + hex(0.5) + " " + hex(0.0F) + " " + hex(0.0F);
        String tick = "1 " + hex(0.0F) + " " + hex(0.0F) + " - " + hex(0.5) + " " + hex(-60.0) + " " + hex(0.5);
        Map<String, List<String>> plans = new TreeMap<>();
        plans.put("ok", List.of("format=etas-plan 1", "account=" + user, start, tick));
        plans.put("ok-alt-account", List.of("format=etas-plan 1", "account=deftenvy", start, tick));
        plans.put("bad-format", List.of("format=etas-plan 3", start, tick));
        String seg = "@segment s 2";
        String tick2 = "2 " + hex(0.0F) + " " + hex(0.0F) + " - " + hex(0.5) + " " + hex(-60.0) + " " + hex(0.5);
        String vel = "@branch 2 set_velocity " + hex(0.0) + " " + hex(0.5) + " " + hex(0.0) + " s";
        plans.put("ok-v2", List.of("format=etas-plan 2", "velocity_tol=1e-6", start, tick, vel, seg, tick2));
        plans.put("v1-segment", List.of("format=etas-plan 1", start, tick, seg, tick2));
        plans.put("v2-unknown-at", List.of("format=etas-plan 2", start, "@frob 1", tick));
        plans.put("v2-no-velocity-tol", List.of("format=etas-plan 2", start, tick, vel, seg, tick2));
        plans.put("v2-no-child", List.of("format=etas-plan 2", "velocity_tol=1e-6", start, tick, vel));
        plans.put("v2-child-start", List.of("format=etas-plan 2", "velocity_tol=1e-6", start, tick, vel, "@segment s 3"));
        plans.put("v2-unreachable", List.of("format=etas-plan 2", start, tick, seg, tick2));
        plans.put("v2-ambiguous", List.of("format=etas-plan 2", "velocity_tol=1e-6", start, tick, vel,
                "@branch 2 set_velocity " + hex(0.0) + " " + hex(0.5 + 1e-6) + " " + hex(0.0) + " t", seg, tick2, "@segment t 2", tick2));
        plans.put("v2-server-line", List.of("format=etas-plan 2", start, "@server 1 glide_off", tick));
        plans.put("unknown-at", List.of("format=etas-plan 1", start, "@branch 1 2 x y", tick));
        plans.put("unknown-event", List.of("format=etas-plan 1", start, "@react 1 1 landing press_jump", tick));
        plans.put("plain-with-react", List.of(start, "@react 1 1 glide_off press_jump", tick));
        plans.put("account", List.of("format=etas-plan 1", "account=other", start, tick));
        if (user.equals(Playback.ALT)) {
            plans.put("main-account", List.of("format=etas-plan 1", "account=lqmbda", start, tick));
        }
        plans.put("ticks", List.of("format=etas-plan 1", start, "ticks=2", tick));
        plans.put("no-start", List.of("format=etas-plan 1", tick));
        Path dir = context.computeOnClient(Playback::dir);
        for (Map.Entry<String, List<String>> e : plans.entrySet()) write(dir.resolve("refuse-" + e.getKey() + ".tas"), e.getValue());
        for (String name : plans.keySet()) {
            String why = context.computeOnClient(c -> {
                String r = Playback.start(c, "refuse-" + name, new Playback.Options(1, 1, 1));
                Playback.stop("refusal test");
                return r;
            });
            boolean shouldStart = allowed && name.startsWith("ok");
            check(shouldStart == (why == null), user + ": refuse-" + name + ": "
                    + (why == null ? "started" : why));
        }
        if (allowed) {
            String why = context.computeOnClient(c -> {
                c.options.sprintWindow().set(7);
                String r = Playback.start(c, "refuse-ok", new Playback.Options(1, 1, 1));
                Playback.stop("refusal test");
                c.options.sprintWindow().set(0);
                return r;
            });
            check(why != null && why.contains("sprint window"), "refuse-ok at sprint window 7: " + (why == null ? "started" : why));
        }
    }

    // --- (a) and (c): a plan from an oracle case ----------------------------------------------

    /** An oracle case's input and its log's poses, as a plan's lines. */
    record OracleCase(String name, String kind, long seed, double[] start, float startYaw, float startPitch,
                      float[] yaw, float[] pitch, int[] keys, double[][] pos) {
        static OracleCase read(Path dir) {
            if (!Files.exists(dir.resolve("oracle.tsv"))) return null;
            try {
                Map<String, String> meta = new HashMap<>();
                TreeMap<Integer, String[]> pre = new TreeMap<>();
                for (String l : Files.readAllLines(dir.resolve("oracle.tsv"))) {
                    String[] f = l.split("\t");
                    if (f[0].equals("M")) {
                        for (int i = 1; i < f.length; i++) meta.put(f[i].substring(0, f[i].indexOf('=')), f[i].substring(f[i].indexOf('=') + 1));
                    } else if (f[0].equals("P")) {
                        pre.put(Integer.parseInt(f[1]), f);
                    }
                }
                TreeMap<Integer, double[]> end = new TreeMap<>();
                double[] start = null;
                float sy = 0, sp = 0;
                try (BufferedReader r = new BufferedReader(new InputStreamReader(new GZIPInputStream(Files.newInputStream(dir.resolve("log.tsv.gz")))))) {
                    for (String l; (l = r.readLine()) != null; ) {
                        String[] f = l.split("\t");
                        if (!f[0].equals("T")) continue;
                        int t = Integer.parseInt(f[2]);
                        double[] p = {Double.parseDouble(f[4]), Double.parseDouble(f[5]), Double.parseDouble(f[6])};
                        if (f[3].equals("e")) end.put(t, p);
                        else if (t == 1) {
                            start = p;
                            sy = (float) Double.parseDouble(f[7]);
                            sp = (float) Double.parseDouble(f[8]);
                        }
                    }
                }
                int n = pre.size();
                float[] yaw = new float[n + 1], pitch = new float[n + 1];
                int[] keys = new int[n + 1];
                double[][] pos = new double[n + 1][];
                pos[0] = start;
                yaw[0] = sy;
                pitch[0] = sp;
                for (int i = 1; i <= n; i++) {
                    String[] f = pre.get(i);
                    if (f == null || !end.containsKey(i)) throw new IllegalStateException(dir + ": no P or T e record for tick " + i);
                    yaw[i] = (float) Double.parseDouble(f[2]);
                    pitch[i] = (float) Double.parseDouble(f[3]);
                    keys[i] = Integer.parseInt(f[4]);
                    if ((keys[i] & Driver.SHIFT) != 0) throw new IllegalStateException(dir + ": tick " + i + " sneaks; plans have no shift key");
                    pos[i] = end.get(i);
                }
                return new OracleCase(dir.getFileName().toString(), meta.get("kind"), Long.parseLong(meta.get("seed")), start, sy, sp,
                        yaw, pitch, keys, pos);
            } catch (IOException e) {
                throw new UncheckedIOException(e);
            }
        }

        int lines() {
            return yaw.length - 1;
        }
    }

    private static List<String> planLines(String source, double[] start, float sy, float sp, float[] yaw, float[] pitch,
                                          int[] keys, double[][] pos, List<String> directives, int perturbLine, double perturb) {
        List<String> l = new ArrayList<>();
        l.add("# written by the playback gametest");
        l.add("format=etas-plan 1");
        l.add("account=" + Playback.ALT);
        l.add("source=" + source);
        l.add("start=" + hex(start[0]) + " " + hex(start[1]) + " " + hex(start[2]) + " " + hex(sy) + " " + hex(sp));
        l.add("ticks=" + (yaw.length - 1));
        l.addAll(directives);
        for (int i = 1; i < yaw.length; i++) {
            double x = pos[i][0] + (i == perturbLine ? perturb : 0.0);
            l.add(i + " " + hex(yaw[i]) + " " + hex(pitch[i]) + " " + Plan.keyString(keys[i] & ~Driver.SHIFT) + " "
                    + hex(x) + " " + hex(pos[i][1]) + " " + hex(pos[i][2]));
        }
        return l;
    }

    private Scenario scenario;

    private void prepareOracleWorld(ClientGameTestContext context, TestSingleplayerContext sp, OracleCase oc) {
        Scenario sc = Scenario.create(oc.kind(), oc.seed());
        scenario = sc;
        if (sc.start.x != oc.start()[0] || sc.start.y != oc.start()[1] || sc.start.z != oc.start()[2]) {
            throw new IllegalStateException(oc.name() + ": the generator's start " + sc.start + " isn't the log's");
        }
        sp.getServer().runOnServer(server -> {
            ServerLevel level = server.overworld();
            for (Map.Entry<BlockPos, String> e : sc.blocks.entrySet()) level.setBlock(e.getKey(), parse(e.getValue()), Block.UPDATE_CLIENTS);
        });
        target = new Target(sc.start, oc.startYaw(), sc.platform);
        // to the start once, and wait for the client to have the blocks, before playback's own resets
        sp.getServer().runOnServer(server -> {
            ServerPlayer p = server.getPlayerList().getPlayers().getFirst();
            if (sc.platform != null) server.overworld().setBlock(sc.platform, Blocks.GLASS.defaultBlockState(), Block.UPDATE_CLIENTS);
            p.setDeltaMovement(Vec3.ZERO);
            p.teleportTo(server.overworld(), sc.start.x, sc.start.y, sc.start.z, Set.of(), oc.startYaw(), 0.0F, true);
        });
        for (int i = 0; ; i++) {
            if (i > 1200) throw new IllegalStateException(oc.name() + ": client never had the case's blocks");
            context.waitTick();
            boolean ready = context.computeOnClient(c -> c.player.onGround() && c.player.position().equals(sc.start)
                    && (sc.platform == null || c.level.getBlockState(sc.platform).is(Blocks.GLASS)));
            if (ready && i > 40) break;
        }
        // every placed block as the server has it
        Map<BlockPos, BlockState> server = sp.getServer().computeOnServer(s -> {
            Map<BlockPos, BlockState> m = new HashMap<>();
            for (BlockPos p : sc.blocks.keySet()) m.put(p, s.overworld().getBlockState(p));
            return m;
        });
        context.waitFor(c -> server.entrySet().stream().allMatch(e -> c.level.getBlockState(e.getKey()).equals(e.getValue())), 1200);
    }

    /** (e) the queue: a plan with no finish gets an answer and the queue goes on; a refused plan stops it. */
    private void caseE(ClientGameTestContext context, OracleCase oc) {
        Path dir = context.computeOnClient(PlanQueue::dir);
        try {
            if (Files.exists(dir)) {
                try (var walk = Files.walk(dir)) {
                    for (Path p : walk.sorted(java.util.Comparator.reverseOrder()).toList()) Files.delete(p);
                }
            }
            Files.createDirectories(dir);
            context.runOnClient(c -> c.player.connection.sendCommand("etas queue on"));
            context.waitTicks(2);
            check(context.computeOnClient(c -> PlanQueue.on()), "(e) /etas queue on");
            List<String> plan = planLines("queue test", oc.start(), oc.startYaw(), oc.startPitch(), oc.yaw(), oc.pitch(),
                    oc.keys(), oc.pos(), List.of(), -1, 0.0);
            removeOnLine1 = scenario.platform;
            drop(dir, "p1", plan);
            Path answer = dir.resolve("done/p1.json");
            for (int i = 0; i < 4000 && !Files.exists(answer); i++) context.waitTick();
            removeOnLine1 = null;
            check(Files.exists(answer), "(e) p1 answered");
            String a = Files.readString(answer);
            check(a.contains("\"ok\": false") && a.contains("without") && a.contains("\"abort_kind\": \"\""), "(e) a plan with no finish: " + a);
            check(Files.exists(dir.resolve("done/p1.tas")) && !Files.exists(dir.resolve("p1.tas")), "(e) p1 moved to done");
            check(context.computeOnClient(c -> PlanQueue.on()), "(e) the queue stays on after a plan with no finish");
            List<String> bad = new ArrayList<>(plan);
            bad.removeIf(l -> l.startsWith("start="));
            drop(dir, "p2-bad", bad);
            drop(dir, "p3", plan);
            answer = dir.resolve("done/p2-bad.json");
            for (int i = 0; i < 200 && !Files.exists(answer); i++) context.waitTick();
            check(Files.exists(answer) && Files.readString(answer).contains("refused"), "(e) a bad plan is refused");
            context.waitTicks(30);
            check(!context.computeOnClient(c -> PlanQueue.on()), "(e) a refusal turns the queue off");
            check(Files.exists(dir.resolve("p3.tas")) && !Files.exists(dir.resolve("done/p3.json")), "(e) later plans stay unflown");
            check(Files.readString(dir.resolve("state")).startsWith("off refused"), "(e) state says why");
            summary.add("(e) queue: " + a.replace('\n', ' '));
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    /** (f) the frame limiter stays off on every tick of a flight and comes back after; a free camera refuses and stops. */
    private void caseF(ClientGameTestContext context, TestSingleplayerContext sp, OracleCase oc) {
        Path dir = context.computeOnClient(Playback::dir);
        String name = "limiter-" + oc.name();
        write(dir.resolve(name + ".tas"), planLines("limiter test", oc.start(), oc.startYaw(), oc.startPitch(), oc.yaw(), oc.pitch(),
                oc.keys(), oc.pos(), List.of(), -1, 0.0));
        InactivityFpsLimit limitBefore = context.computeOnClient(c -> c.options.inactivityFpsLimit().get());
        context.runOnClient(c -> {
            c.options.inactivityFpsLimit().set(InactivityFpsLimit.AFK);
            markIdle(c);
        });
        check(context.computeOnClient(PlaybackGameTest::throttled), "(f) idle is throttled before the flight");
        int[] throttled = {0}, ticks = {0};
        removeOnLine1 = scenario.platform;
        // why mark idle again each tick? then each check shows that tick's own reset, not one left over from the start
        Playback.Status s = play(context, sp, "etas play " + name + " runs=1", oc.lines(), 4000, st -> {
            if (context.computeOnClient(PlaybackGameTest::throttled)) throttled[0]++;
            ticks[0]++;
            context.runOnClient(PlaybackGameTest::markIdle);
        });
        removeOnLine1 = null;
        check(s.history().size() == 1 && s.history().getFirst().ok(), "(f) the flight finished; stop: " + s.stopReason());
        check(throttled[0] == 0, "(f) throttled on " + throttled[0] + " of " + ticks[0] + " ticks");
        context.runOnClient(PlaybackGameTest::markIdle);
        context.waitTicks(3);
        check(context.computeOnClient(PlaybackGameTest::throttled), "(f) idle is throttled again after the flight");

        context.runOnClient(c -> c.setCameraEntity(new ArmorStand(c.level, c.player.getX(), c.player.getY(), c.player.getZ())));
        String why = context.computeOnClient(c -> Playback.start(c, name, new Playback.Options(1, 10, 1)));
        check(why != null && why.contains("camera"), "(f) a free camera refuses a plan: " + why);
        context.runOnClient(c -> c.setCameraEntity(c.player));
        context.runOnClient(c -> Playback.start(c, name, new Playback.Options(1, 10, 1)));
        context.waitTicks(2);
        check(context.computeOnClient(c -> Playback.active()), "(f) the plan starts with the camera back");
        context.runOnClient(c -> c.setCameraEntity(new ArmorStand(c.level, c.player.getX(), c.player.getY(), c.player.getZ())));
        context.waitTicks(2);
        Playback.Status stopped = context.computeOnClient(c -> Playback.status());
        check(stopped.phase() == Playback.Phase.IDLE && String.valueOf(stopped.stopReason()).contains("camera"),
                "(f) a free camera stops a plan: " + stopped.stopReason());
        context.runOnClient(c -> {
            c.setCameraEntity(c.player);
            c.options.inactivityFpsLimit().set(limitBefore);
        });
    }

    private static boolean throttled(Minecraft c) {
        return c.getFramerateLimitTracker().getThrottleReason() == FramerateLimitTracker.FramerateThrottleReason.LONG_AFK;
    }

    /** As if the last input were past the 10-minute limit. */
    private static void markIdle(Minecraft c) {
        try {
            java.lang.reflect.Field f = FramerateLimitTracker.class.getDeclaredField("latestInputTime");
            f.setAccessible(true);
            f.setLong(c.getFramerateLimitTracker(), Util.getMillis() - 700_000L);
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException(e);
        }
    }

    private static void drop(Path dir, String name, List<String> plan) throws IOException {
        Path tmp = dir.resolve(name + ".tmp");
        write(tmp, plan);
        Files.move(tmp, dir.resolve(name + ".tas"), StandardCopyOption.ATOMIC_MOVE);
    }

    private void caseA(ClientGameTestContext context, TestSingleplayerContext sp, OracleCase oc, Path out) {
        Path dir = context.computeOnClient(Playback::dir);
        String name = "oracle-" + oc.name();
        List<String> plan = planLines("oracle case " + oc.name(), oc.start(), oc.startYaw(), oc.startPitch(), oc.yaw(), oc.pitch(),
                oc.keys(), oc.pos(), List.of(), -1, 0.0);
        write(dir.resolve(name + ".tas"), plan);
        write(out.resolve(name + ".tas"), plan);
        removeOnLine1 = scenario.platform;
        Path log = startLog(context);
        Playback.Status s = play(context, sp, "etas play " + name + " runs=2", oc.lines(), 4000, null);
        removeOnLine1 = null;
        Path kept = keepLog(context, log, out.resolve(name + ".log.tsv.gz"));
        List<Playback.Attempt> h = s.history();
        check(h.size() == 2 && h.stream().allMatch(Playback.Attempt::ok),
                "(a) " + name + ": " + h.size() + " attempts, " + h.stream().filter(Playback.Attempt::ok).count() + " ok; stop: " + s.stopReason()
                        + (h.isEmpty() ? "" : "; first: " + h.getFirst().reason()));
        for (Playback.Attempt a : h) {
            summary.add(String.format("(a) %s attempt %d: %s, %d lines, %d divergent ticks (max %.3g, first at line %d)",
                    name, a.n(), a.reason(), a.lines(), a.divergences(), a.maxDev(), a.firstDivergence()));
        }
        summary.add("(a) log " + kept + ": " + recordCounts(kept));
    }

    private void caseC(ClientGameTestContext context, TestSingleplayerContext sp, OracleCase oc, Path out) {
        int nudgeAt = Math.min(120, oc.lines() - 40), perturbAt = Math.min(150, oc.lines() - 30);
        Path dir = context.computeOnClient(Playback::dir);
        String name = "oracle-" + oc.name() + "-diverge";
        // attempt 1: the player is moved 1e-3 in x on the tick line nudgeAt is applied, so the
        // check before nudgeAt + 1 aborts; attempt 2: the plan's own line perturbAt is 1e-2 off
        List<String> plan = planLines("oracle case " + oc.name() + ", line " + perturbAt + " x + 0.01", oc.start(), oc.startYaw(),
                oc.startPitch(), oc.yaw(), oc.pitch(), oc.keys(), oc.pos(), List.of(), perturbAt, 0.01);
        write(dir.resolve(name + ".tas"), plan);
        write(out.resolve(name + ".tas"), plan);
        removeOnLine1 = scenario.platform;
        nudgeLine = nudgeAt;
        nudgeAttempt = 1;
        nudge = 1e-3;
        atAbort = null;
        nudged = null;
        List<String> aborts = new ArrayList<>();
        Path log = startLog(context);
        Playback.Status s = play(context, sp, "etas play " + name + " runs=1 maxfail=2", oc.lines(), 4000, st -> {
            if (atAbort != null) {
                aborts.add(atAbort);
                atAbort = null;
            }
        });
        nudgeLine = -1;
        removeOnLine1 = null;
        Path kept = keepLog(context, log, out.resolve(name + ".log.tsv.gz"));
        List<Playback.Attempt> h = s.history();
        boolean shape = h.size() == 2 && !h.get(0).ok() && !h.get(1).ok();
        check(shape && h.get(0).abortLine() == nudgeAt && Math.abs(h.get(0).abortDev() - 1e-3) < 1e-6,
                "(c) nudged 1e-3 at line " + nudgeAt + " (" + nudged + "): " + (h.isEmpty() ? "no attempt" : describe(h.get(0))));
        check(shape && h.get(1).abortLine() == perturbAt && Math.abs(h.get(1).abortDev() - 1e-2) < 1e-9 && h.get(1).divergences() == 1,
                "(c) plan line " + perturbAt + " off by 1e-2: " + (h.size() < 2 ? "no second attempt" : describe(h.get(1))));
        check(s.stopReason().equals("2 failures in a row"), "(c) stopped: " + s.stopReason());
        // attempt 1 resets and goes on with playback's input; attempt 2 stops and hands the input back
        check(aborts.size() == 2 && aborts.stream().allMatch(a -> a.contains(": keys 0 move 0.0,0.0 restored ") && a.endsWith("autoJump 0"))
                        && aborts.get(0).contains("restored false") && aborts.get(1).contains("restored true"),
                "(c) input on the abort ticks: " + aborts);
        boolean restored = context.computeOnClient(c -> c.player.input == driverInput);
        check(restored, "(c) the player's input is back to what it was before playback");
        // the abort tick's physics ran with no keys: the logger's END record of that tick
        List<String> abortTicks = releasedInLog(kept);
        check(abortTicks.size() == 2 && abortTicks.stream().allMatch(t -> t.endsWith("input 0")), "(c) logged abort ticks: " + abortTicks);
        summary.add("(c) " + describe(h.get(0)) + "; " + (h.size() > 1 ? describe(h.get(1)) : ""));
        summary.add("(c) log " + kept + ": " + recordCounts(kept));

        // the same nudge after "Course complete!" (sent FINISH_BEFORE_END lines before the end) is a success
        String after = "oracle-" + oc.name() + "-after-finish";
        List<String> clean = planLines("oracle case " + oc.name() + ", nudged after the finish", oc.start(), oc.startYaw(),
                oc.startPitch(), oc.yaw(), oc.pitch(), oc.keys(), oc.pos(), List.of(), -1, 0.0);
        write(dir.resolve(after + ".tas"), clean);
        write(out.resolve(after + ".tas"), clean);
        removeOnLine1 = scenario.platform;
        nudgeLine = oc.lines() - FINISH_BEFORE_END + 10;
        nudgeAttempt = 1;
        Playback.Status sa = play(context, sp, "etas play " + after + " runs=1 maxfail=1", oc.lines(), 4000, null);
        int nudgedAt = nudgeLine;
        nudgeLine = -1;
        removeOnLine1 = null;
        List<Playback.Attempt> ha = sa.history();
        check(ha.size() == 1 && ha.get(0).ok() && ha.get(0).finishLine() >= 0 && ha.get(0).finishLine() < nudgedAt
                        && ha.get(0).lines() <= nudgedAt + 1,
                "(c) nudged after the finish, at line " + nudgedAt + ": " + (ha.isEmpty() ? "no attempt" : describe(ha.get(0))));
        summary.add("(c) after the finish: " + (ha.isEmpty() ? "no attempt" : describe(ha.get(0))));
    }

    private static String describe(Playback.Attempt a) {
        return String.format("attempt %d %s, abort line %d off %.6g, %d divergent ticks (first %d): %s", a.n(), a.ok() ? "ok" : "failed",
                a.abortLine(), a.abortDev(), a.divergences(), a.firstDivergence(), a.reason());
    }

    // --- (b) ebounces ------------------------------------------------------------------------

    /**
     * Sprint off the edge of a pillar and deploy, dive straight out for {@link #STRAIGHT}
     * ticks, then turn in a circle that misses the pillar, gliding into the flat world's
     * ground and pitching up for a hop after each landing. No keys while gliding.
     */
    static final class Bounce implements Driver.Policy {
        static final float TURN = 2.0F, DIVE = 50.0F, APPROACH = 20.0F, HOP = -25.0F;
        static final int HOP_TICKS = 5, STRAIGHT = 40;
        final float yaw0;
        final double floor;
        final List<Driver.Step> steps = new ArrayList<>();
        /** Ticks that ended on the ground while gliding. */
        final List<Integer> ground = new ArrayList<>();
        private float yaw;
        private int hop, air;
        private boolean dived;

        Bounce(float yaw0, double floor) {
            this.yaw0 = yaw0;
            this.yaw = yaw0;
            this.floor = floor;
        }

        @Override
        public Driver.Step next(LocalPlayer p, int i) {
            int keys = 0;
            float pitch;
            if (i > STRAIGHT) yaw += TURN;
            if (!p.isFallFlying()) {
                pitch = 0.0F;
                // a jump press in air, 2 ticks after leaving the pillar, deploys
                air = p.onGround() ? 0 : air + 1;
                keys = Driver.W | Driver.SPRINT | (air == 2 ? Driver.J : 0);
            } else if (p.onGround()) {
                ground.add(i - 1);
                hop = HOP_TICKS - 1;
                pitch = HOP;
            } else if (hop > 0) {
                hop--;
                pitch = HOP;
            } else if (!dived && p.getY() > floor + 4) {
                pitch = DIVE;
            } else {
                dived = true;
                pitch = APPROACH;
            }
            Driver.Step s = new Driver.Step(yaw, pitch, keys);
            steps.add(s);
            return s;
        }
    }

    private void caseB(ClientGameTestContext context, TestSingleplayerContext sp, Path out) {
        int length = Integer.parseInt(System.getenv().getOrDefault("PLAYBACK_BOUNCE_TICKS", "240"));
        int runs = Integer.parseInt(System.getenv().getOrDefault("PLAYBACK_BOUNCE_RUNS", "9"));
        int height = 50;
        double floor = -60.0;
        BlockPos base = new BlockPos(0, -60, 0);
        // at the pillar's +z edge, facing +z
        Vec3 start = new Vec3(0.37, floor + height, 0.93);
        float yaw0 = 0.0F;
        sp.getServer().runOnServer(server -> {
            for (int y = 0; y < height; y++) server.overworld().setBlock(base.above(y), Blocks.GLASS.defaultBlockState(), Block.UPDATE_CLIENTS);
        });
        target = new Target(start, yaw0, null);
        teleport(sp, start, yaw0);
        // the path stays within about 100 blocks of the start; the client loads a circle of chunks
        for (int i = 0; ; i++) {
            String missing = context.computeOnClient(c -> {
                int n = 0;
                String first = null;
                for (int cx = -9; cx <= 9; cx++)
                    for (int cz = -9; cz <= 9; cz++) {
                        if (cx * cx + cz * cz > 49 || c.level.getChunkSource().hasChunk(cx, cz)) continue;
                        if (n++ == 0) first = cx + "," + cz;
                    }
                if (!c.level.getBlockState(base.above(height - 1)).is(Blocks.GLASS)) return "the pillar";
                return n == 0 ? null : n + " chunks, e.g. " + first + "; player in " + c.player.chunkPosition()
                        + ", render distance " + c.options.renderDistance().get();
            });
            if (missing == null) break;
            if (i == 600) {
                summary.add("(b) the client never had " + missing + "; going on");
                break;
            }
            context.waitTick();
        }
        settle(context, sp, start, yaw0);

        // the held run: Minecraft with nothing ending the glide
        Bounce policy = new Bounce(yaw0, floor);
        holdGlide = true;
        Path heldLog = startLog(context);
        context.runOnClient(c -> Driver.begin(policy));
        for (int i = 0; i < length; i++) {
            context.waitTick();
            String err = context.computeOnClient(c -> Driver.error());
            if (err != null) throw new IllegalStateException("(b) held run: " + err);
        }
        int steps = context.computeOnClient(c -> {
            Driver.end();
            return Driver.steps();
        });
        context.runOnClient(c -> NetLog.stop("held run"));
        holdGlide = false;
        List<String> records = List.copyOf(Driver.records());
        List<double[]> positions = List.copyOf(Driver.positions());
        List<Integer> landings = List.copyOf(policy.ground);
        boolean flying = context.computeOnClient(c -> c.player.isFallFlying());
        check(flying, "(b) held run still gliding at its end");
        // single-tick contacts: the glide-off arrives 3 ticks after a landing in singleplayer,
        // and a restart needs the player in the air by then
        List<Integer> bounces = new ArrayList<>();
        List<Integer> longer = new ArrayList<>();
        for (int k = 0; k < landings.size(); k++) {
            int L = landings.get(k);
            boolean next = k + 1 < landings.size() && landings.get(k + 1) == L + 1;
            boolean prev = k > 0 && landings.get(k - 1) == L - 1;
            if (prev) continue;
            if (next) longer.add(L);
            else bounces.add(L);
        }
        double[] lo = {1e9, 1e9, 1e9}, hi = {-1e9, -1e9, -1e9};
        for (double[] p : positions) {
            for (int k = 0; k < 3; k++) {
                lo[k] = Math.min(lo[k], p[k]);
                hi[k] = Math.max(hi[k], p[k]);
            }
        }
        summary.add(String.format("(b) held run: %d ticks in x %.0f..%.0f, z %.0f..%.0f, %d single-tick landings at %s%s",
                steps, lo[0], hi[0], lo[2], hi[2], bounces.size(), bounces, longer.isEmpty() ? "" : ", longer contacts from " + longer));

        // the held run as an oracle case, for port's replay (no glide-off: port keeps gliding)
        Path caseDir = out.resolve("cases").resolve("ebounce-held");
        writeCase(sp, caseDir, heldLog, records, positions, steps, base, height);

        // the plan: the held run's input and poses, a reaction after each landing, up to the
        // first contact longer than a tick (the glide-off then finds the player on the ground,
        // where nothing can answer it)
        int lines = longer.isEmpty() ? steps : longer.getFirst();
        float[] yaw = new float[lines + 1], pitch = new float[lines + 1];
        int[] keys = new int[lines + 1];
        double[][] pos = new double[lines + 1][];
        pos[0] = new double[]{start.x, start.y, start.z};
        yaw[0] = yaw0;
        for (int i = 1; i <= lines; i++) {
            Driver.Step s = policy.steps.get(i - 1);
            yaw[i] = s.yaw();
            pitch[i] = s.pitch();
            keys[i] = s.keys();
            pos[i] = positions.get(i - 1);
        }
        List<String> reacts = new ArrayList<>();
        for (int L : bounces) if (L < lines) reacts.add("@react " + (L + 1) + " " + Math.min(L + 6, lines) + " glide_off press_jump");
        Path dir = context.computeOnClient(Playback::dir);
        List<String> plan = planLines("held ebounce run", pos[0], yaw[0], 0.0F, yaw, pitch, keys, pos, reacts, -1, 0.0);
        List<String> bare = planLines("held ebounce run, no reactions", pos[0], yaw[0], 0.0F, yaw, pitch, keys, pos, List.of(), -1, 0.0);
        write(dir.resolve("ebounce.tas"), plan);
        write(out.resolve("ebounce.tas"), plan);
        write(dir.resolve("ebounce-bare.tas"), bare);

        // the control: without reactions the vanilla glide-off ends the glide, and the run leaves the plan
        Path bareLog = startLog(context);
        Playback.Status c0 = play(context, sp, "etas play ebounce-bare runs=1 maxfail=1", lines, 2000, null);
        keepLog(context, bareLog, out.resolve("ebounce-bare.log.tsv.gz"));
        Playback.Attempt b0 = c0.history().isEmpty() ? null : c0.history().getFirst();
        int first = bounces.isEmpty() ? -1 : bounces.getFirst();
        check(b0 != null && !b0.ok() && b0.abortLine() > first && b0.abortLine() <= first + 10,
                "(b) without reactions: " + (b0 == null ? "no attempt" : describe(b0) + "; glide-offs before lines " + b0.glideOffs())
                        + "; first landing " + first);

        // the mod's own hold: the same plan flies with no glide-off to react to
        Playback.Status c1 = play(context, sp, "etas play ebounce-bare runs=1 maxfail=1 hold_glide=1", lines, 2000, null);
        Playback.Attempt b1 = c1.history().isEmpty() ? null : c1.history().getFirst();
        check(b1 != null && b1.ok() && b1.divergences() == 0 && b1.glideOffs().isEmpty(),
                "(b) hold_glide without reactions: " + (b1 == null ? "no attempt" : describe(b1) + "; glide-offs before lines " + b1.glideOffs()));

        // with them, several runs (which also shows the hold is gone: they need the vanilla glide-off)
        Path log = startLog(context);
        Playback.Status s = play(context, sp, "etas play ebounce runs=" + runs + " maxfail=2", lines, (lines + 80) * (runs + 2), null);
        Path kept = keepLog(context, log, out.resolve("ebounce.log.tsv.gz"));
        List<Playback.Attempt> h = s.history();
        check(h.size() == runs && h.stream().allMatch(Playback.Attempt::ok),
                "(b) " + h.size() + " attempts, " + h.stream().filter(Playback.Attempt::ok).count() + " ok; stop " + s.stopReason()
                        + (h.isEmpty() ? "" : "; " + describe(h.getLast())));
        // arrival: the line a glide-off was first seen before, after each landing
        TreeMap<Integer, Integer> spread = new TreeMap<>();
        int fired = 0, expected = 0, divergent = 0;
        double maxDev = 0.0;
        for (Playback.Attempt a : h) {
            divergent += a.divergences();
            maxDev = Math.max(maxDev, a.maxDev());
            fired += a.reactions().size();
            for (int L : bounces) {
                // the glide-off comes 3 ticks after the landing in singleplayer
                if (L + 3 > a.lines()) continue;
                expected++;
                for (int g : a.glideOffs()) {
                    if (g > L && g <= L + 8) {
                        spread.merge(g - L, 1, Integer::sum);
                        break;
                    }
                }
            }
        }
        check(fired == expected && divergent == 0, String.format("(b) %d of %d landings reacted to; %d divergent ticks, max %.3g",
                fired, expected, divergent, maxDev));
        summary.add("(b) glide-off first seen before line landing + k, k: count = " + spread + " over " + expected + " landings in "
                + h.size() + " runs");
        summary.add("(b) log " + kept + ": " + recordCounts(kept));
        summary.add("(b) held case for port: " + caseDir);
    }

    // --- (d) branches ------------------------------------------------------------------------

    /** A Driver run: the steps it took and the pose after each, line 1 first. */
    private record Rec(List<Driver.Step> steps, List<double[]> pos) {
    }

    /** Records every step a policy takes. */
    private static final class Recorded implements Driver.Policy {
        final Driver.Policy inner;
        final List<Driver.Step> steps = new ArrayList<>();

        Recorded(Driver.Policy inner) {
            this.inner = inner;
        }

        @Override
        public Driver.Step next(LocalPlayer p, int i) {
            Driver.Step s = inner.next(p, i);
            steps.add(s);
            return s;
        }
    }

    /** From standing still at {@code start}: {@code n} steps of {@code pol}, with {@code inj} handed to the client. */
    private Rec record(ClientGameTestContext context, TestSingleplayerContext sp, Vec3 start, float yaw, Driver.Policy pol, int n,
                       List<Inject> inj) {
        teleport(sp, start, yaw);
        settle(context, sp, start, yaw);
        Recorded r = new Recorded(pol);
        injected.clear();
        injects = inj;
        recording = true;
        context.runOnClient(c -> Driver.begin(r));
        try {
            for (int i = 0; i < n; i++) {
                context.waitTick();
                String err = context.computeOnClient(c -> Driver.error());
                if (err != null) throw new IllegalStateException("(d) recording: " + err);
            }
        } finally {
            context.runOnClient(c -> Driver.end());
            recording = false;
            injects = List.of();
        }
        return new Rec(List.copyOf(r.steps), List.copyOf(Driver.positions()));
    }

    /** Tick lines {@code from..to} of a recording. */
    private static List<String> lines(Rec r, int from, int to) {
        List<String> l = new ArrayList<>();
        for (int t = from; t <= to; t++) {
            Driver.Step s = r.steps().get(t - 1);
            double[] p = r.pos().get(t - 1);
            l.add(t + " " + hex(s.yaw()) + " " + hex(s.pitch()) + " " + Plan.keyString(s.keys() & ~Driver.SHIFT) + " "
                    + hex(p[0]) + " " + hex(p[1]) + " " + hex(p[2]));
        }
        return l;
    }

    private static String velocityBranch(int t, double[] v, String child) {
        return "@branch " + t + " set_velocity " + hex(v[0]) + " " + hex(v[1]) + " " + hex(v[2]) + " " + child;
    }

    /** First tick (from {@code from}) at which two recordings' poses differ, or -1. */
    private static int firstDifference(Rec a, Rec b, int from, int to) {
        for (int t = from; t <= to; t++) if (!java.util.Arrays.equals(a.pos().get(t - 1), b.pos().get(t - 1))) return t;
        return -1;
    }

    /** One {@code /etas play NAME runs=1 maxfail=1} with {@code inj} handed to the client. */
    private Playback.Attempt attemptWith(ClientGameTestContext context, TestSingleplayerContext sp, String name, int lines, List<Inject> inj) {
        injected.clear();
        injects = inj;
        try {
            Playback.Status s = play(context, sp, "etas play " + name + " runs=1 maxfail=1", lines, (lines + 200) * 2, null);
            return s.history().isEmpty() ? null : s.history().getLast();
        } finally {
            injects = List.of();
        }
    }

    private void expect(String what, Playback.Attempt a, boolean ok, String path, String abortKind, boolean exact) {
        boolean good = a != null && a.ok() == ok && String.join(" ", a.path()).equals(path) && a.abortKind().equals(abortKind)
                && (!exact || a.divergences() == 0);
        String got = a == null ? "no attempt" : String.format("%s, path %s, %s, %d divergent (max %.3g), velocities before %s, glide-offs before %s",
                a.ok() ? "ok" : "failed (" + a.abortKind() + ")", String.join(" ", a.path()), a.reason(), a.divergences(),
                a.maxDev(), a.velocities(), a.glideOffs());
        check(good, "(d) " + what + ": " + got + "; injected " + injected);
        summary.add("(d) " + what + ": " + got);
    }

    /**
     * Velocity branches, standing on the flat world's ground: up 0.6 or 0.4 before line 20,
     * or 0.6 before 21 and again before 22 (a booster that re-sends), all rejoining J at
     * line 50 once they have landed (then the states are the same bit for bit); the root
     * is the path with nothing. J walks forward on lines 70-89 and ignores velocities
     * before lines 60-69.
     */
    private void caseDVelocity(ClientGameTestContext context, TestSingleplayerContext sp, Path out) {
        int n = 120, t1 = 20, join = 50;
        Vec3 start = new Vec3(20.5, -60.0, -20.5);
        float yaw0 = 0.0F;
        target = new Target(start, yaw0, null);
        Driver.Policy walk = (p, i) -> new Driver.Step(yaw0, 0.0F, i >= 70 && i < 90 ? Driver.W : 0);
        double[] up6 = {0.0, 0.6, 0.0}, up4 = {0.0, 0.4, 0.0};
        Rec r0 = record(context, sp, start, yaw0, walk, n, List.of());
        Rec ra = record(context, sp, start, yaw0, walk, n, List.of(new Inject(t1, "velocity", up6)));
        Rec rb = record(context, sp, start, yaw0, walk, n, List.of(new Inject(t1, "velocity", up4)));
        Rec rc = record(context, sp, start, yaw0, walk, n, List.of(new Inject(t1 + 1, "velocity", up6), new Inject(t1 + 2, "velocity", up6)));
        // what the plan's structure assumes: the same lines until the injection, the same from the rejoin
        check(firstDifference(r0, ra, 1, t1 - 1) < 0 && firstDifference(r0, rc, 1, t1) < 0, "(d) recordings agree before the injections");
        check(firstDifference(r0, ra, t1, t1) == t1 && firstDifference(ra, rb, t1, t1) == t1, "(d) the injections move the player");
        int[] rejoin = {firstDifference(r0, ra, join, n), firstDifference(r0, rb, join, n), firstDifference(r0, rc, join, n)};
        check(rejoin[0] < 0 && rejoin[1] < 0 && rejoin[2] < 0, "(d) A, B and C2 meet the root's poses from line " + join
                + " (first difference, -1 = none): " + java.util.Arrays.toString(rejoin));
        double air = 0;
        for (int t = t1; t < join; t++) air = Math.max(air, ra.pos().get(t - 1)[1] - start.y);
        summary.add(String.format("(d) velocity recordings: A rises %.3f blocks; all four stand still from line %d", air, join));

        List<String> plan = new ArrayList<>(List.of("# written by the playback gametest, (d)", "format=etas-plan 2",
                "account=" + Playback.ALT, "source=branches", "start=" + hex(start.x) + " " + hex(start.y) + " " + hex(start.z) + " "
                        + hex(yaw0) + " " + hex(0.0F), "ticks=" + (n + 2 * (join - t1) + 1 + (join - t1 - 2) + (n - join + 1)),
                "velocity_tol=1e-6"));
        List<String> root = lines(r0, 1, n);
        root.addAll(t1 - 1, List.of(velocityBranch(t1, up6, "A"), velocityBranch(t1, up4, "B"), velocityBranch(t1 + 1, up6, "C")));
        plan.addAll(root);
        plan.add("@segment A " + t1);
        plan.addAll(lines(ra, t1, join - 1));
        plan.add("@goto J");
        plan.add("@segment B " + t1);
        plan.addAll(lines(rb, t1, join - 1));
        plan.add("@goto J");
        plan.add("@segment C " + (t1 + 1));
        plan.addAll(lines(rc, t1 + 1, t1 + 1));
        plan.add(velocityBranch(t1 + 2, up6, "C2"));
        plan.add("@segment C2 " + (t1 + 2));
        plan.addAll(lines(rc, t1 + 2, join - 1));
        plan.add("@goto J");
        plan.add("@segment J " + join);
        plan.add("@ignore 60 69 set_velocity");
        plan.addAll(lines(r0, join, n));
        Path dir = context.computeOnClient(Playback::dir);
        write(dir.resolve("branches.tas"), plan);
        write(out.resolve("branches.tas"), plan);

        Path log = startLog(context);
        expect("nothing", attemptWith(context, sp, "branches", n, List.of()), true, "root@1", "", true);
        expect("0.6 before 20", attemptWith(context, sp, "branches", n, List.of(new Inject(t1, "velocity", up6))), true,
                "root@1 A@20 J@50", "", true);
        expect("0.4 before 20 (rejoins)", attemptWith(context, sp, "branches", n, List.of(new Inject(t1, "velocity", up4))), true,
                "root@1 B@20 J@50", "", true);
        expect("0.6 before 21 and 22", attemptWith(context, sp, "branches", n, List.of(new Inject(t1 + 1, "velocity", up6),
                new Inject(t1 + 2, "velocity", up6))), true, "root@1 C@21 C2@22 J@50", "", true);
        expect("0.6 + 5e-7 before 20 (within velocity_tol)", attemptWith(context, sp, "branches", n,
                List.of(new Inject(t1, "velocity", new double[]{0.0, 0.6 + 5e-7, 0.0}))), true, "root@1 A@20 J@50", "", false);
        expect("0.6 before 20, its own velocity before 65 (@ignore in J)", attemptWith(context, sp, "branches", n,
                List.of(new Inject(t1, "velocity", up6), new Inject(65, "velocity", null))), true, "root@1 A@20 J@50", "", true);
        expect("0.6 before 21 only (missing)", attemptWith(context, sp, "branches", n, List.of(new Inject(t1 + 1, "velocity", up6))),
                false, "root@1 C@21", "missing", true);
        expect("0.5 before 20 (unlisted payload)", attemptWith(context, sp, "branches", n, List.of(new Inject(t1, "velocity",
                new double[]{0.0, 0.5, 0.0}))), false, "root@1", "unlisted", true);
        expect("0.6 before 20, its own velocity before 55 (unlisted: J ignores 60-69)", attemptWith(context, sp, "branches", n,
                List.of(new Inject(t1, "velocity", up6), new Inject(55, "velocity", null))), false, "root@1 A@20 J@50", "unlisted", true);
        Path kept = keepLog(context, log, out.resolve("branches.log.tsv.gz"));
        summary.add("(d) log " + kept + ": " + recordCounts(kept));
    }

    /** Glides off a pillar and around a circle. */
    static final class Circle implements Driver.Policy {
        static final float TURN = 3.0F, PITCH = 10.0F;
        final float yaw0;
        private float yaw;
        private int air;

        Circle(float yaw0) {
            this.yaw0 = yaw0;
            this.yaw = yaw0;
        }

        @Override
        public Driver.Step next(LocalPlayer p, int i) {
            if (i > 10) yaw += TURN;
            if (p.isFallFlying()) return new Driver.Step(yaw, PITCH, 0);
            // a jump press in air, 2 ticks after leaving the pillar, deploys; after a glide-off
            // the count is past 2, so it walks on without redeploying
            air = p.onGround() ? (air > 2 ? air : 0) : air + 1;
            return new Driver.Step(yaw, 0.0F, Driver.W | Driver.SPRINT | (air == 2 ? Driver.J : 0));
        }
    }

    /**
     * Glide-off branches while gliding: the glide-off handed to the client before line 35
     * or 36 goes to G or G2 (the fall without the glide); before 30 nothing takes it.
     */
    private void caseDGlide(ClientGameTestContext context, TestSingleplayerContext sp, Path out) {
        int n = 80, g = 35, height = 80;
        BlockPos base = new BlockPos(-40, -60, -40);
        sp.getServer().runOnServer(server -> {
            for (int y = 0; y < height; y++) server.overworld().setBlock(base.above(y), Blocks.GLASS.defaultBlockState(), Block.UPDATE_CLIENTS);
        });
        Vec3 start = new Vec3(base.getX() + 0.37, base.getY() + height, base.getZ() + 0.93);
        float yaw0 = 0.0F;
        target = new Target(start, yaw0, null);
        context.waitFor(c -> c.level.getBlockState(base.above(height - 1)).is(Blocks.GLASS), 1200);
        Rec r0 = record(context, sp, start, yaw0, new Circle(yaw0), n, List.of());
        Rec rg = record(context, sp, start, yaw0, new Circle(yaw0), n, List.of(new Inject(g, "glide_off", null)));
        Rec rg2 = record(context, sp, start, yaw0, new Circle(yaw0), n, List.of(new Inject(g + 1, "glide_off", null)));
        check(firstDifference(r0, rg, 1, g - 1) < 0 && firstDifference(r0, rg2, 1, g) < 0 && firstDifference(r0, rg, g, g) == g,
                "(d) glide recordings agree before the glide-off and part at it");
        double lowest = 1e9;
        for (double[] p : r0.pos()) lowest = Math.min(lowest, p[1]);
        summary.add(String.format("(d) glide recordings: root lowest y %.2f; G at line %d y %.2f; injected %s", lowest, n,
                rg.pos().get(n - 1)[1], injected));

        List<String> plan = new ArrayList<>(List.of("# written by the playback gametest, (d)", "format=etas-plan 2",
                "account=" + Playback.ALT, "source=glide branches", "start=" + hex(start.x) + " " + hex(start.y) + " " + hex(start.z)
                        + " " + hex(yaw0) + " " + hex(0.0F), "ticks=" + (n + (n - g + 1) + (n - g))));
        List<String> root = lines(r0, 1, n);
        root.addAll(g - 1, List.of("@branch " + g + " glide_off G", "@branch " + (g + 1) + " glide_off G2"));
        plan.addAll(root);
        plan.add("@segment G " + g);
        plan.addAll(lines(rg, g, n));
        plan.add("@segment G2 " + (g + 1));
        plan.addAll(lines(rg2, g + 1, n));
        Path dir = context.computeOnClient(Playback::dir);
        write(dir.resolve("glide-branches.tas"), plan);
        write(out.resolve("glide-branches.tas"), plan);

        Path log = startLog(context);
        expect("no glide-off", attemptWith(context, sp, "glide-branches", n, List.of()), true, "root@1", "", true);
        expect("glide-off before 35", attemptWith(context, sp, "glide-branches", n, List.of(new Inject(g, "glide_off", null))), true,
                "root@1 G@35", "", true);
        expect("glide-off before 36", attemptWith(context, sp, "glide-branches", n, List.of(new Inject(g + 1, "glide_off", null))), true,
                "root@1 G2@36", "", true);
        expect("glide-off before 30 (unlisted)", attemptWith(context, sp, "glide-branches", n, List.of(new Inject(30, "glide_off", null))),
                false, "root@1", "unlisted", true);
        Path kept = keepLog(context, log, out.resolve("glide-branches.log.tsv.gz"));
        summary.add("(d) log " + kept + ": " + recordCounts(kept));
    }

    private static void teleport(TestSingleplayerContext sp, Vec3 pos, float yaw) {
        sp.getServer().runOnServer(server -> {
            ServerPlayer p = server.getPlayerList().getPlayers().getFirst();
            p.setDeltaMovement(Vec3.ZERO);
            p.teleportTo(server.overworld(), pos.x, pos.y, pos.z, Set.of(), yaw, 0.0F, true);
        });
    }

    /** Standing still at {@code pos}, not gliding, for 20 ticks. */
    private static void settle(ClientGameTestContext context, TestSingleplayerContext sp, Vec3 pos, float yaw) {
        int still = 0;
        for (int i = 0; still < 20; i++) {
            if (i > 600) throw new IllegalStateException("never stood still at " + pos);
            if (i % 100 == 99) teleport(sp, pos, yaw);
            context.waitTick();
            boolean ok = context.computeOnClient(c -> c.player.onGround() && !c.player.isFallFlying()
                    && c.player.position().equals(pos) && c.player.getDeltaMovement().horizontalDistanceSqr() == 0.0);
            still = ok ? still + 1 : 0;
        }
    }

    /** The held run in the oracle's case format (solver/oracle/README.md, "Case files"). */
    private static void writeCase(TestSingleplayerContext sp, Path dir, Path log, List<String> records, List<double[]> positions,
                                  int steps, BlockPos base, int height) {
        // down to the bedrock: port's collision scan reaches a cell below the grass
        int[] box = {Integer.MAX_VALUE, -64, Integer.MAX_VALUE, Integer.MIN_VALUE, Integer.MIN_VALUE, Integer.MIN_VALUE};
        List<double[]> pts = new ArrayList<>(positions);
        pts.add(new double[]{base.getX(), base.getY() + height, base.getZ()});
        for (double[] p : pts) {
            box[0] = Math.min(box[0], (int) Math.floor(p[0]) - 8);
            box[2] = Math.min(box[2], (int) Math.floor(p[2]) - 8);
            box[3] = Math.max(box[3], (int) Math.floor(p[0]) + 8);
            box[4] = Math.max(box[4], (int) Math.floor(p[1]) + 8);
            box[5] = Math.max(box[5], (int) Math.floor(p[2]) + 8);
        }
        Map<BlockPos, String> world = sp.getServer().computeOnServer(server -> scan(server.overworld(), box));
        try {
            Files.createDirectories(dir);
            Files.move(log, dir.resolve("log.tsv.gz"), StandardCopyOption.REPLACE_EXISTING);
            List<String> lines = new ArrayList<>();
            lines.add("#etas-oracle\t1");
            lines.add(String.join("\t", "M", "kind=ebounce_held", "seed=0", "steps=" + steps, "stop=length",
                    "origin=" + base.getX() + "," + base.getY() + "," + base.getZ(), "structure=pillar", "blocks_changed=0",
                    "mc=" + SharedConstants.getCurrentVersion().name(),
                    "java=" + System.getProperty("java.vm.vendor").replace(' ', '_') + "_" + System.getProperty("java.vm.version"),
                    "os=" + System.getProperty("os.name").replace(' ', '_') + "_" + System.getProperty("os.arch")));
            lines.add(String.join("\t", "A", "" + box[0], "" + box[1], "" + box[2], "" + box[3], "" + box[4], "" + box[5]));
            for (Map.Entry<BlockPos, String> e : world.entrySet()) {
                BlockPos p = e.getKey();
                lines.add(String.join("\t", "B", "" + p.getX(), "" + p.getY(), "" + p.getZ(), e.getValue()));
            }
            lines.addAll(records);
            Files.write(dir.resolve("oracle.tsv"), lines);
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    /** Every non-air block in the box (getChunk loads what isn't). */
    private static Map<BlockPos, String> scan(ServerLevel level, int[] box) {
        Map<BlockPos, String> m = new TreeMap<>((a, b) -> a.getY() != b.getY() ? Integer.compare(a.getY(), b.getY())
                : a.getZ() != b.getZ() ? Integer.compare(a.getZ(), b.getZ()) : Integer.compare(a.getX(), b.getX()));
        for (int cx = box[0] >> 4; cx <= box[3] >> 4; cx++) {
            for (int cz = box[2] >> 4; cz <= box[5] >> 4; cz++) {
                LevelChunk chunk = level.getChunk(cx, cz);
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

    // --- logs ------------------------------------------------------------------------------

    private static Map<String, Integer> recordCounts(Path log) {
        Map<String, Integer> n = new TreeMap<>();
        for (String[] f : records(log)) {
            if (f[0].equals("P")) n.merge(f[3], 1, Integer::sum);
        }
        return n;
    }

    /** For each P abort record: its tick and the input bits of that tick's END record. */
    private static List<String> releasedInLog(Path log) {
        List<String[]> recs = records(log);
        List<String> out = new ArrayList<>();
        for (String[] f : recs) {
            if (!f[0].equals("P") || !f[3].equals("abort")) continue;
            String tick = f[2];
            String input = "?";
            for (String[] g : recs) {
                if (g[0].equals("T") && g[2].equals(tick) && g[3].equals("e")) input = g[16];
            }
            out.add("tick " + tick + " " + f[4] + " input " + input);
        }
        return out;
    }

    private static List<String[]> records(Path log) {
        List<String[]> out = new ArrayList<>();
        try (BufferedReader r = new BufferedReader(new InputStreamReader(new GZIPInputStream(Files.newInputStream(log))))) {
            for (String l; (l = r.readLine()) != null; ) {
                if (!l.startsWith("#")) out.add(l.split("\t"));
            }
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
        return out;
    }

    // --- small things ------------------------------------------------------------------------

    private static void write(Path file, List<String> lines) {
        try {
            Files.createDirectories(file.getParent());
            Files.write(file, lines);
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    private static String hex(float v) {
        return String.format("%08x", Float.floatToRawIntBits(v));
    }

    private static String hex(double v) {
        return String.format("%016x", Double.doubleToRawLongBits(v));
    }

    private static final Map<String, BlockState> PARSED = new HashMap<>();

    private static BlockState parse(String state) {
        return PARSED.computeIfAbsent(state, s -> {
            try {
                return BlockStateParser.parseForBlock(net.minecraft.core.registries.BuiltInRegistries.BLOCK, s, false).blockState();
            } catch (com.mojang.brigadier.exceptions.CommandSyntaxException e) {
                throw new IllegalArgumentException(s, e);
            }
        });
    }
}
