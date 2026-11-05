package jealoustone.elytratas.play;

import jealoustone.elytratas.ElytraTasClient;
import jealoustone.elytratas.mixin.ClientInputAccessor;
import jealoustone.elytratas.mixin.LocalPlayerAccessor;
import jealoustone.elytratas.netlog.NetLog;
import net.minecraft.client.Minecraft;
import net.minecraft.client.multiplayer.ServerData;
import net.minecraft.client.player.ClientInput;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.network.chat.Component;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.player.Input;
import net.minecraft.world.phys.Vec2;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.IdentityHashMap;
import java.util.List;
import java.util.Set;

/**
 * {@code /etas play}: flies a plan (solver/PLAN-FORMAT.md version 2 or 1) open loop, in a
 * loop of attempts, as tas_play.pyj does, switching segments on what the server sends
 * (the spec's "Playback": {@link #play}).
 *
 * <ul>
 * <li>Inputs are set in {@code START_LEVEL_TICK} ({@link #step}), at the head of
 * {@code ClientLevel.tickEntities}, right before the local player's physics: rotation with
 * {@code setYRot}/{@code setXRot} from the plan's float bits, keys in a plain
 * {@link ClientInput} swapped in for {@code player.input} for the whole run, so real keys,
 * toggle-sprint and screens can't touch it. A jump press is sent as {@code autoJumpTime = 1}
 * with jump off: a plain ClientInput's {@code tick()} is empty, so {@code aiStep} would
 * read a jump set before it as {@code wasJumping} and never see an edge (README.md, "Things
 * that will bite"). The oracle's Driver does the same, and the oracle checks port against it.</li>
 * <li>Observations come from {@code ClientPacketListenerMixin}: a glide-off ({@link #glideOff})
 * or a velocity ({@link #setVelocity}) the client just handled, both at the start of a
 * frame, before its ticks; the next {@link #step} takes them as observed before the line
 * it sets.</li>
 * <li>The camera is smoothed without touching physics: {@link #frame} sets the rotation
 * every frame, before camera setup, to the lerp between the line physics just used and
 * the next one; {@link #tickStart} puts the exact value back at the head of every tick,
 * and {@link #step} sets the next line's before physics. So physics and the moves sent to
 * the server only ever see plan values. The mouse doesn't turn the player while a plan runs.</li>
 * </ul>
 * Client thread only.
 */
public final class Playback {
    /** Multiplayer playback is restricted to the approved alt; the main may play in singleplayer. */
    public static final String ALT = "deftenvy";

    public static boolean accountAllowed(String user, boolean singleplayer) {
        return ALT.equals(user) || (singleplayer && "lqmbda".equals(user));
    }
    /** Ticks to keep flying the plan after "Course complete!", as tas_play. */
    static final int POST_FINISH = 20;
    /** Ticks between reset-item uses while not at the start, and how many uses before giving up. */
    private static final int RESET_RETRY = 40, RESET_USES = 4;
    /** Hotbar slot selected once at the start: keeps the reset item out of hand, as the recording did (tas_play). */
    private static final int PLAY_SLOT = 3;

    public enum Phase { IDLE, RESET, SETTLE, PLAY }

    /**
     * runs 0 = until stopped; settle = still ticks at the start before line 1; maxFail = failed attempts in a row before
     * stopping. holdGlide is a test hack, never for a real run: see {@link #holdsGlide}.
     */
    public record Options(int runs, int settle, int maxFail, boolean holdGlide) {
        public static final Options DEFAULT = new Options(0, 10, 3);

        public Options(int runs, int settle, int maxFail) {
            this(runs, settle, maxFail, false);
        }
    }

    /**
     * One finished attempt. {@code reactions}, {@code glideOffs} and {@code velocities} are
     * plan lines: the line a reaction fired on (its index among the plan's reactions in
     * {@code reactionDirectives}), and every line a glide-off or a velocity was first seen
     * before. {@code path} is the segments flown, each as {@code name@line} with the line
     * it was entered on. {@code abortKind} is "" for a success or a plan that ended
     * without the finish, else diverge, rotation, unlisted or missing.
     */
    public record Attempt(int n, boolean ok, String reason, int lines, int divergences, double maxDev,
                          int firstDivergence, int abortLine, double abortDev, List<Integer> reactions,
                          List<Integer> reactionDirectives, List<Integer> glideOffs, int finishLine,
                          List<Integer> velocities, List<String> path, String abortKind) {
    }

    public record Status(Phase phase, String plan, int lines, int line, int attempt, int done, int runs, int fails,
                         int divergences, double maxDev, int reactions, String stopReason, List<Attempt> history,
                         String segment) {
    }

    /** Volatile: {@link #holdsGlide} reads it on the integrated server's thread. */
    private static volatile Phase phase = Phase.IDLE;
    private static Plan plan;
    private static Options opts = Options.DEFAULT;
    private static LocalPlayer player;
    private static ClientInput orig;
    private static ClientInput input;
    private static String stopReason = "";
    private static final List<Attempt> HISTORY = new ArrayList<>();

    private static int attempt, done, fails, uses, wait;
    private static boolean hub;
    // per attempt
    private static int applied;
    private static boolean jumpDown;
    private static boolean pendingGlideOff;
    /** The last velocity handled since the last step, and how many. */
    private static double[] pendingVelocity;
    private static int pendingVelocities;
    /** The segment that flies the next line, and the one that flew line {@code applied}. */
    private static Plan.Segment seg;
    private static Set<Plan.React> fired;
    private static int divergences, firstDivergence;
    private static double maxDev;
    private static int finishLine;
    private static String finishText = "";
    private static List<Integer> reactions, reactionDirectives, glideOffs, velocities;
    private static List<String> path;
    // camera
    private static boolean camera;
    private static boolean moved;
    private static float exactYaw, exactPitch, shownYaw, shownPitch;

    private Playback() {
    }

    public static Path dir(Minecraft mc) {
        return mc.gameDirectory.toPath().resolve("elytra-tas").resolve("plans");
    }

    /**
     * Integrated server's thread: whether to keep this player's glide through a landing,
     * which is what port models, so a plan's main line flies with no glide-off to react to.
     * A test hack: {@link #start} refuses it outside an unpublished singleplayer world, and
     * only the attempt itself holds, since a reset has to wait for the glide flag to clear.
     */
    public static boolean holdsGlide(ServerPlayer p) {
        LocalPlayer local = player;
        return phase == Phase.PLAY && opts.holdGlide() && local != null && p.getUUID().equals(local.getUUID());
    }

    public static boolean active() {
        return phase != Phase.IDLE;
    }

    public static Status status() {
        return new Status(phase, plan == null ? null : plan.name, plan == null ? 0 : plan.lastTick, applied, attempt,
                done, opts.runs(), fails, divergences, maxDev, reactions == null ? 0 : reactions.size(), stopReason,
                List.copyOf(HISTORY), seg == null ? null : seg.name);
    }

    /** Starts playing; null on success, else why it refused. */
    public static String start(Minecraft mc, String name, Options o) {
        if (phase != Phase.IDLE) return "already playing " + plan.name + " (/etas stop)";
        LocalPlayer p = mc.player;
        if (p == null || mc.gameMode == null || mc.getConnection() == null) return "not in a world";
        String user = mc.getUser().getName();
        boolean singleplayer = mc.getSingleplayerServer() != null;
        // why? the hold is nonvanilla movement on the server: only ever on our own unpublished world
        if (o.holdGlide() && (!singleplayer || mc.getSingleplayerServer().isPublished())) {
            return "hold_glide is for testing in an unpublished singleplayer world only";
        }
        if (!accountAllowed(user, singleplayer)) return "refusing to drive " + user
                + ": playback only drives " + ALT + " in multiplayer, or lqmbda in singleplayer (AGENTS.md)";
        // why? plans come from port, which has no double-tap sprint; another window can sprint a tape's W presses
        int sprintWindow = mc.options.sprintWindow().get();
        if (sprintWindow != 0) return "sprint window is " + sprintWindow + ", plans assume 0 (Options → Controls → Sprint Window)";
        // why? playback is input no vanilla client makes; it goes only where the logger may send its own packets
        ServerData server = mc.getCurrentServer();
        if (!singleplayer && (server == null || !ElytraTasClient.CONFIG.whitelisted(user, server.ip))) {
            return user + "@" + (server == null ? "?" : server.ip) + " isn't whitelisted (Mod Menu → Elytra TAS → Whitelist)";
        }
        if (!name.matches("[A-Za-z0-9_.+-]+") || name.startsWith(".")) return "bad plan name " + name;
        String stem = name.endsWith(".tas") ? name.substring(0, name.length() - 4) : name;
        Path file = dir(mc).resolve(stem + ".tas");
        if (!Files.isRegularFile(file)) return "no plan at " + file;
        Plan pl;
        try {
            pl = Plan.read(file, stem);
        } catch (Plan.PlanException e) {
            return "refusing " + stem + ": " + e.getMessage();
        }
        String account = pl.get("account");
        boolean mainPlayingAltPlan = singleplayer && "lqmbda".equals(user) && ALT.equals(account);
        if (account != null && !account.equals(user) && !mainPlayingAltPlan) {
            return "refusing " + stem + ": it is for account=" + account + ", this is " + user;
        }

        if (!NetLog.active() && ElytraTasClient.CONFIG.enabled) {
            NetLog.start(mc.getConnection().getConnection(), server, mc);
        }
        plan = pl;
        opts = o;
        player = p;
        orig = p.input;
        input = new ClientInput();
        p.input = input;
        setKeys(0, false);
        HISTORY.clear();
        attempt = done = fails = 0;
        hub = false;
        stopReason = "";
        applied = 0;
        divergences = 0;
        maxDev = 0.0;
        reactions = new ArrayList<>();
        NetLog.playback("start", "plan=" + stem, "lines=" + pl.lastTick, "reactions=" + pl.reactions.size(),
                "tol=" + pl.tol, "runs=" + o.runs(), "settle=" + o.settle(), "maxfail=" + o.maxFail(),
                "format=" + pl.format(), "segments=" + pl.segments.size(), "branches=" + pl.branchCount,
                "tick_lines=" + pl.tickLines, "velocity_tol=" + pl.velocityTol, "hold_glide=" + (o.holdGlide() ? 1 : 0));
        say("playing " + stem + ": " + pl.lastTick + " ticks, " + pl.reactions.size() + " reactions"
                + (pl.segments.size() > 1 ? ", " + pl.segments.size() + " segments, " + pl.branchCount + " branches" : "")
                + ", tolerance " + pl.tol
                + (o.holdGlide() ? " (hold_glide: test only, the server keeps the glide, not a real run)" : "")
                + (NetLog.active() ? "" : " (the logger is off: nothing is recorded)") + ". The keyboard and mouse don't move you; /etas stop.");
        beginAttempt(mc);
        return null;
    }

    public static void stop(String reason) {
        if (phase == Phase.IDLE) return;
        phase = Phase.IDLE;
        camera = false;
        stopReason = reason;
        Minecraft mc = Minecraft.getInstance();
        if (mc.player == player && player != null) {
            setKeys(0, false);
            if (player.input == input) player.input = orig;
        }
        player = null;
        orig = null;
        NetLog.playback("stop", "reason=" + reason, "done=" + done, "attempts=" + attempt);
        say("stopped: " + reason + ". " + done + " runs done in " + attempt + " attempts.");
    }

    // --- per tick -------------------------------------------------------------------

    /** START_CLIENT_TICK: put back the rotation physics used, before anything in the tick reads it. */
    public static void tickStart(Minecraft mc) {
        if (!camera || mc.player != player) return;
        checkMoved();
        setRot(exactYaw, exactPitch);
    }

    /** Each frame, before camera setup (Minecraft.renderFrame head): the smoothed rotation. */
    public static void frame(Minecraft mc) {
        if (!camera || mc.player != player) return;
        try {
            checkMoved();
            float alpha = Math.min(mc.getDeltaTracker().getGameTimeDeltaPartialTick(false), 1.0F);
            // toward the segment's next line; the first line of a branch not taken yet isn't known
            Plan.Segment g = seg;
            int k = g.i(applied), n = g.has(applied + 1) ? k + 1 : k;
            setRot(g.yaw[k] + (g.yaw[n] - g.yaw[k]) * alpha, g.pitch[k] + (g.pitch[n] - g.pitch[k]) * alpha);
        } catch (RuntimeException e) {
            // never into the render loop; step() stops on the flag
            moved = true;
        }
    }

    /** START_LEVEL_TICK: right before this tick's player physics. */
    public static void step(Minecraft mc) {
        if (phase == Phase.IDLE) return;
        try {
            stepInner(mc);
        } catch (RuntimeException e) {
            NetLog.LOGGER.error("elytra-tas: playback failed", e);
            stop("error: " + e);
        }
    }

    private static void stepInner(Minecraft mc) {
        LocalPlayer p = mc.player;
        if (p == null || p != player) {
            stop("player changed (respawn, dimension change, or disconnect)");
            return;
        }
        if (p.input != input) {
            stop("player.input was replaced by something else");
            return;
        }
        if (p.isDeadOrDying()) {
            stop("player died");
            return;
        }
        if (hub) {
            stop("sent to hub");
            return;
        }

        switch (phase) {
            case RESET -> {
                clearPending();
                if (atStart(p)) {
                    phase = Phase.SETTLE;
                    wait = 0;
                    p.getInventory().setSelectedSlot(PLAY_SLOT);
                    return;
                }
                if (++wait % RESET_RETRY == 0) useResetItem(mc);
            }
            case SETTLE -> {
                clearPending();
                if (!atStart(p)) {
                    phase = Phase.RESET;
                    wait = 0;
                    useResetItem(mc);
                    return;
                }
                if (++wait < opts.settle()) return;
                // the plan assumes the reset's rotation for its first still ticks
                if (p.getYRot() != plan.yaw0 || p.getXRot() != plan.pitch0) setRot(plan.yaw0, plan.pitch0);
                phase = Phase.PLAY;
                applied = 0;
                jumpDown = false;
                seg = plan.root;
                path.add(seg.name + "@1");
                NetLog.playback("play", "attempt=" + attempt);
                // line 1: nothing observed yet, but a root that branches or ends at 1 says so here
                if (!advance(1)) return;
                moved = false;
                camera = true;
            }
            case PLAY -> play(p);
            default -> {
            }
        }
    }

    private static void play(LocalPlayer p) {
        // the last physics tick ran line `applied` of `seg`; check its pose
        int i = applied;
        double dx = p.getX() - seg.x[seg.i(i)], dy = p.getY() - seg.y[seg.i(i)], dz = p.getZ() - seg.z[seg.i(i)];
        double d = Math.sqrt(dx * dx + dy * dy + dz * dz);
        if (d != 0.0) {
            divergences++;
            if (firstDivergence < 0) firstDivergence = i;
            NetLog.playback("diverge", "line=" + i, "dx=" + dx, "dy=" + dy, "dz=" + dz, "d=" + d, "seg=" + seg.name);
        }
        maxDev = Math.max(maxDev, d);
        // the run is timed: a landing or wall echo after it isn't the plan's, and isn't a failure
        if (finishLine >= 0 && (d > plan.tol || moved || p.getYRot() != exactYaw || p.getXRot() != exactPitch)) {
            NetLog.playback("post_finish", "line=" + i, "dx=" + dx, "dy=" + dy, "dz=" + dz, "d=" + d, "seg=" + seg.name);
            endAttempt(true, "complete", -1, 0.0, "");
            return;
        }
        if (moved || p.getYRot() != exactYaw || p.getXRot() != exactPitch) {
            endAttempt(false, "rotation changed outside playback at line " + i, i, d, "rotation");
            return;
        }
        if (d > plan.tol) {
            NetLog.playback("abort", "line=" + i, "dx=" + dx, "dy=" + dy, "dz=" + dz, "d=" + d, "tol=" + plan.tol);
            endAttempt(false, String.format("off the plan by %.3g blocks after line %d (tolerance %.3g)", d, i, plan.tol), i, d,
                    "diverge");
            return;
        }
        if (finishLine >= 0 && i >= finishLine + POST_FINISH) {
            endAttempt(true, "complete", -1, 0.0, "");
            return;
        }
        advance(i + 1);
    }

    /**
     * Steps 2-5 of the spec's Playback for line {@code t}: the observations since the last
     * tick choose the segment, then line {@code t} is set. False if the attempt ended.
     */
    private static boolean advance(int t) {
        boolean glide = pendingGlideOff;
        double[] vel = pendingVelocity;
        int nvel = pendingVelocities;
        clearPending();
        if (glide) glideOffs.add(t);
        if (vel != null) {
            velocities.add(t);
            NetLog.playback("set_velocity", "line=" + t, "vx=" + vel[0], "vy=" + vel[1], "vz=" + vel[2], "n=" + nvel,
                    "seg=" + seg.name);
        }
        // branches, then the goto at the segment's end, as long as one applies
        while (true) {
            Plan.Branch b = match(t, glide, vel);
            if (b != null) {
                if (b.event() == Plan.Event.GLIDE_OFF) glide = false;
                else vel = null;
                NetLog.playback("branch", "line=" + t, "event=" + b.event().id, "from=" + seg.name, "to=" + b.child());
                seg = seg.child(b);
                path.add(seg.name + "@" + t);
                continue;
            }
            if (t == seg.end() && seg.next != null) {
                NetLog.playback("goto", "line=" + t, "from=" + seg.name, "to=" + seg.next.name);
                seg = seg.next;
                path.add(seg.name + "@" + t);
                continue;
            }
            break;
        }
        // what no branch took: a reaction, an @ignore, or (version 2) an abort
        Plan.React react = null;
        String unlisted = null;
        if (glide) {
            for (Plan.React r : seg.reactions) {
                if (!fired.contains(r) && r.event().equals("glide_off") && r.t0() <= t && t <= r.t1()) {
                    fired.add(r);
                    react = r;
                    break;
                }
            }
            boolean ignored = react == null && ignored(Plan.Event.GLIDE_OFF, t);
            NetLog.playback("glide_off", "line=" + t, "react=" + (react == null ? "-" : Integer.toString(react.index())),
                    "seg=" + seg.name, "taken=" + (react != null ? "react" : ignored ? "ignore" : "-"));
            if (react != null) {
                reactions.add(t);
                reactionDirectives.add(react.index());
                NetLog.playback("react", "line=" + t, "directive=" + react.index(), "event=glide_off", "action=press_jump");
            } else if (!ignored) {
                unlisted = "glide_off";
            }
        }
        if (vel != null && !ignored(Plan.Event.SET_VELOCITY, t)) {
            unlisted = unlisted == null ? "set_velocity " + vel[0] + "," + vel[1] + "," + vel[2]
                    : unlisted + " and set_velocity " + vel[0] + "," + vel[1] + "," + vel[2];
        } else if (vel != null) {
            NetLog.playback("ignore", "line=" + t, "event=set_velocity", "seg=" + seg.name);
        }
        if (unlisted != null && plan.version == 2) {
            if (finishLine >= 0) {
                // the run is over and timed; what the server does after that isn't the plan's
                endAttempt(true, "complete", -1, 0.0, "");
                return false;
            }
            NetLog.playback("unlisted", "line=" + t, "event=" + unlisted, "seg=" + seg.name);
            endAttempt(false, "unlisted " + unlisted + " before line " + t + " in segment " + seg.name, t - 1, 0.0, "unlisted");
            return false;
        }
        if (t == seg.end()) {
            if (seg.branchesAt(t)) {
                StringBuilder want = new StringBuilder();
                for (Plan.Branch b : seg.branches) if (b.tick() == t) want.append(want.isEmpty() ? "" : " or ").append(b.event().id);
                NetLog.playback("missing", "line=" + t, "seg=" + seg.name, "want=" + want);
                endAttempt(false, "no " + want + " before line " + t + " in segment " + seg.name, t - 1, 0.0, "missing");
            } else if (finishLine >= 0) {
                endAttempt(true, "complete", -1, 0.0, "");
            } else {
                endAttempt(false, "plan ended without \"Course complete!\"", -1, 0.0, "");
            }
            return false;
        }
        apply(t, react != null);
        return true;
    }

    /** The current segment's branch at {@code t} that an untaken observation matches, or null. */
    private static Plan.Branch match(int t, boolean glide, double[] vel) {
        for (Plan.Branch b : seg.branches) {
            if (b.tick() != t) continue;
            if (b.event() == Plan.Event.GLIDE_OFF && glide) return b;
            if (b.event() == Plan.Event.SET_VELOCITY && vel != null) {
                boolean ok = true;
                for (int k = 0; k < 3; k++) ok &= Math.abs(vel[k] - b.v()[k]) <= plan.velocityTol;
                if (ok) return b;
            }
        }
        return null;
    }

    private static boolean ignored(Plan.Event e, int t) {
        for (Plan.Ignore g : seg.ignores) if (g.event() == e && g.t0() <= t && t <= g.t1()) return true;
        return false;
    }

    private static void clearPending() {
        pendingGlideOff = false;
        pendingVelocity = null;
        pendingVelocities = 0;
    }

    /**
     * Sets line {@code line}'s rotation and keys for the physics right after. A reaction's
     * press_jump is a jump edge on this tick; the jump then counts as held, so a J on the
     * next line is a hold, not another press.
     */
    private static void apply(int line, boolean pressJump) {
        int j = seg.i(line);
        setRot(seg.yaw[j], seg.pitch[j]);
        exactYaw = shownYaw;
        exactPitch = shownPitch;
        applied = line;
        int k = seg.keys[j];
        boolean press = pressJump || ((k & Plan.J) != 0 && !jumpDown);
        setKeys(press ? k & ~Plan.J : k, press);
        jumpDown = press || (k & Plan.J) != 0;
    }

    // --- attempts ---------------------------------------------------------------------

    private static void beginAttempt(Minecraft mc) {
        attempt++;
        phase = Phase.RESET;
        camera = false;
        wait = 0;
        uses = 0;
        applied = 0;
        divergences = 0;
        firstDivergence = -1;
        maxDev = 0.0;
        finishLine = -1;
        finishText = "";
        fired = Collections.newSetFromMap(new IdentityHashMap<>());
        reactions = new ArrayList<>();
        reactionDirectives = new ArrayList<>();
        glideOffs = new ArrayList<>();
        velocities = new ArrayList<>();
        path = new ArrayList<>();
        seg = plan.root;
        clearPending();
        NetLog.playback("attempt", "n=" + attempt);
        useResetItem(mc);
    }

    private static void endAttempt(boolean ok, String reason, int abortLine, double abortDev, String abortKind) {
        camera = false;
        setKeys(0, false);
        jumpDown = false;
        HISTORY.add(new Attempt(attempt, ok, reason, applied, divergences, maxDev, firstDivergence, abortLine, abortDev,
                List.copyOf(reactions), List.copyOf(reactionDirectives), List.copyOf(glideOffs), finishLine,
                List.copyOf(velocities), List.copyOf(path), abortKind));
        NetLog.playback("end", "attempt=" + attempt, "result=" + (ok ? "ok" : "fail"), "reason=" + reason,
                "line=" + applied, "divergences=" + divergences, "max_dev=" + maxDev, "reactions=" + reactions.size(),
                "finish_line=" + finishLine, "path=" + String.join(",", path));
        if (ok) {
            done++;
            fails = 0;
            say(String.format("run %d ok (%s), %d divergent ticks, max %.2e, %d reactions%s", done, finishText, divergences,
                    maxDev, reactions.size(), opts.holdGlide() ? " (hold_glide: test only)" : ""));
        } else {
            fails++;
            say("attempt " + attempt + " failed: " + reason);
        }
        if (opts.runs() > 0 && done >= opts.runs()) stop("finished " + done + " runs");
        else if (fails >= opts.maxFail()) stop(fails + " failures in a row");
        else beginAttempt(Minecraft.getInstance());
    }

    private static boolean atStart(LocalPlayer p) {
        // not gliding either: a reset mid-glide keeps the client's flag until the server's glide-off
        return p.getX() == plan.x0 && p.getY() == plan.y0 && p.getZ() == plan.z0 && !p.isFallFlying();
    }

    /** The slot-1 item: on EMC, "Go Back to Last Checkpoint", which with no checkpoint set is the course start. */
    private static void useResetItem(Minecraft mc) {
        if (uses >= RESET_USES) {
            stop("the reset item didn't bring us to the start");
            return;
        }
        setKeys(0, false);
        player.getInventory().setSelectedSlot(0);
        mc.gameMode.useItem(player, InteractionHand.MAIN_HAND);
        uses++;
    }

    // --- events from mixins -------------------------------------------------------------

    /**
     * The client just handled entity data that cleared the local player's fall-flying flag
     * (the server's glide-off, or an undeploy mod handing one over). It is handled in
     * Minecraft.runTick's processQueuedPackets, at the start of a frame and before that
     * frame's ticks, so the next {@link #step} (START_LEVEL_TICK, the head of
     * tickEntities) still comes before the physics of the first tick to see the flag
     * cleared: pressing there lands the press in that tick's aiStep, where
     * tryToStartFallFlying restarts the glide before travel. Pressing from here directly
     * would be overwritten by that step's keys, and pressing at END_CLIENT_TICK or a
     * render event would be a tick late.
     */
    public static void glideOff() {
        if (phase == Phase.PLAY) pendingGlideOff = true;
    }

    /**
     * The client just handled {@code set_entity_motion} for the local player, which set its
     * velocity to {@code v} ({@code Entity.lerpMotion}). Handled where {@link #glideOff} is,
     * so the same timing holds: the next {@link #step} is before the first physics to use it.
     * Only the last one before a tick counts; the earlier ones were overwritten unflown.
     */
    public static void setVelocity(double vx, double vy, double vz) {
        if (phase != Phase.PLAY) return;
        pendingVelocity = new double[]{vx, vy, vz};
        pendingVelocities++;
    }

    /** A server system message (not an overlay) the client just handled. */
    public static void serverMessage(String text) {
        if (phase == Phase.IDLE) return;
        if (text.startsWith("Teleported to hub")) hub = true;
        if (phase != Phase.PLAY) return;
        if (text.startsWith("Course complete!") && finishLine < 0) {
            finishLine = applied;
            finishText = text;
            NetLog.playback("finish", "line=" + applied, "text=" + text);
        } else if (text.equals("Timer started!")) {
            NetLog.playback("timer", "line=" + applied);
        }
    }

    public static boolean blocksMouse() {
        return phase != Phase.IDLE;
    }

    // --- input ---------------------------------------------------------------------------

    private static void setRot(float yaw, float pitch) {
        player.setYRot(yaw);
        player.setXRot(pitch);
        shownYaw = player.getYRot();
        shownPitch = player.getXRot();
    }

    private static void checkMoved() {
        if (player.getYRot() != shownYaw || player.getXRot() != shownPitch) moved = true;
    }

    private static void setKeys(int keys, boolean press) {
        // cleared otherwise, so stopping right after a scheduled press can't leave a stray jump
        ((LocalPlayerAccessor) player).elytraTas$setAutoJumpTime(press ? 1 : 0);
        input.keyPresses = new Input((keys & Plan.W) != 0, (keys & Plan.S) != 0, (keys & Plan.A) != 0,
                (keys & Plan.D) != 0, (keys & Plan.J) != 0, false, (keys & Plan.SPRINT) != 0);
        // KeyboardInput.tick: Vec2(left impulse, forward impulse).normalized()
        float forward = impulse((keys & Plan.W) != 0, (keys & Plan.S) != 0);
        float left = impulse((keys & Plan.A) != 0, (keys & Plan.D) != 0);
        ((ClientInputAccessor) input).elytraTas$setMoveVector(new Vec2(left, forward).normalized());
    }

    private static float impulse(boolean positive, boolean negative) {
        return positive == negative ? 0.0F : positive ? 1.0F : -1.0F;
    }

    private static void say(String text) {
        Minecraft mc = Minecraft.getInstance();
        if (mc.player != null) mc.player.sendSystemMessage(Component.literal("etas play: " + text));
        NetLog.LOGGER.info("etas play: {}", text);
    }
}
