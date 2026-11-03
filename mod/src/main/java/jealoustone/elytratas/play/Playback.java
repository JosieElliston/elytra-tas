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
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.player.Input;
import net.minecraft.world.phys.Vec2;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/**
 * {@code /etas play}: flies a plan (solver/PLAN-FORMAT.md version 1) open loop, in a loop
 * of attempts, as tas_play.pyj does.
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
 * <li>The camera is smoothed without touching physics: {@link #frame} sets the rotation
 * every frame, before camera setup, to the lerp between the line physics just used and
 * the next one; {@link #tickStart} puts the exact value back at the head of every tick,
 * and {@link #step} sets the next line's before physics. So physics and the moves sent to
 * the server only ever see plan values. The mouse doesn't turn the player while a plan runs.</li>
 * </ul>
 * Client thread only.
 */
public final class Playback {
    /** The only account playback drives. why? AGENTS.md: anything that moves the player runs on the alt. */
    public static final String ALT = "deftenvy";
    /** Ticks to keep flying the plan after "Course complete!", as tas_play. */
    static final int POST_FINISH = 20;
    /** Ticks between reset-item uses while not at the start, and how many uses before giving up. */
    private static final int RESET_RETRY = 40, RESET_USES = 4;
    /** Hotbar slot selected once at the start: keeps the reset item out of hand, as the recording did (tas_play). */
    private static final int PLAY_SLOT = 3;

    public enum Phase { IDLE, RESET, SETTLE, PLAY }

    /** runs 0 = until stopped; settle = still ticks at the start before line 1; maxFail = failed attempts in a row before stopping. */
    public record Options(int runs, int settle, int maxFail) {
        public static final Options DEFAULT = new Options(0, 10, 3);
    }

    /**
     * One finished attempt. {@code reactions} and {@code glideOffs} are plan lines:
     * the line a reaction fired on (its directive index in {@code reactionDirectives}),
     * and every line a glide-off was first seen before.
     */
    public record Attempt(int n, boolean ok, String reason, int lines, int divergences, double maxDev,
                          int firstDivergence, int abortLine, double abortDev, List<Integer> reactions,
                          List<Integer> reactionDirectives, List<Integer> glideOffs, int finishLine) {
    }

    public record Status(Phase phase, String plan, int lines, int line, int attempt, int done, int runs, int fails,
                         int divergences, double maxDev, int reactions, String stopReason, List<Attempt> history) {
    }

    private static Phase phase = Phase.IDLE;
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
    private static boolean[] fired;
    private static int divergences, firstDivergence;
    private static double maxDev;
    private static int finishLine;
    private static String finishText = "";
    private static List<Integer> reactions, reactionDirectives, glideOffs;
    // camera
    private static boolean camera;
    private static boolean moved;
    private static float exactYaw, exactPitch, shownYaw, shownPitch;

    private Playback() {
    }

    public static Path dir(Minecraft mc) {
        return mc.gameDirectory.toPath().resolve("elytra-tas").resolve("plans");
    }

    public static boolean active() {
        return phase != Phase.IDLE;
    }

    public static Status status() {
        return new Status(phase, plan == null ? null : plan.name, plan == null ? 0 : plan.lines, applied, attempt,
                done, opts.runs(), fails, divergences, maxDev, reactions == null ? 0 : reactions.size(), stopReason,
                List.copyOf(HISTORY));
    }

    /** Starts playing; null on success, else why it refused. */
    public static String start(Minecraft mc, String name, Options o) {
        if (phase != Phase.IDLE) return "already playing " + plan.name + " (/etas stop)";
        LocalPlayer p = mc.player;
        if (p == null || mc.gameMode == null || mc.getConnection() == null) return "not in a world";
        String user = mc.getUser().getName();
        if (!ALT.equals(user)) return "refusing to drive " + user + ": playback only drives " + ALT + " (AGENTS.md)";
        // why? playback is input no vanilla client makes; it goes only where the logger may send its own packets
        boolean singleplayer = mc.getSingleplayerServer() != null;
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
        if (account != null && !account.equals(user)) return "refusing " + stem + ": it is for account=" + account + ", this is " + user;

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
        NetLog.playback("start", "plan=" + stem, "lines=" + pl.lines, "reactions=" + pl.reactions.size(),
                "tol=" + pl.tol, "runs=" + o.runs(), "settle=" + o.settle(), "maxfail=" + o.maxFail(),
                "format=" + (pl.versioned ? Plan.FORMAT : "tas"));
        say("playing " + stem + ": " + pl.lines + " ticks, " + pl.reactions.size() + " reactions, tolerance " + pl.tol
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
            int k = applied;
            int n = Math.min(k + 1, plan.lines);
            setRot(plan.yaw[k] + (plan.yaw[n] - plan.yaw[k]) * alpha, plan.pitch[k] + (plan.pitch[n] - plan.pitch[k]) * alpha);
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
                pendingGlideOff = false;
                if (atStart(p)) {
                    phase = Phase.SETTLE;
                    wait = 0;
                    p.getInventory().setSelectedSlot(PLAY_SLOT);
                    return;
                }
                if (++wait % RESET_RETRY == 0) useResetItem(mc);
            }
            case SETTLE -> {
                pendingGlideOff = false;
                if (!atStart(p)) {
                    phase = Phase.RESET;
                    wait = 0;
                    useResetItem(mc);
                    return;
                }
                if (++wait < opts.settle()) return;
                // the plan assumes the reset's rotation for its first still ticks
                if (p.getYRot() != plan.yaw[0] || p.getXRot() != plan.pitch[0]) setRot(plan.yaw[0], plan.pitch[0]);
                phase = Phase.PLAY;
                applied = 0;
                jumpDown = false;
                NetLog.playback("play", "attempt=" + attempt);
                apply(1, false);
                moved = false;
                camera = true;
            }
            case PLAY -> play(p);
            default -> {
            }
        }
    }

    private static void play(LocalPlayer p) {
        // the last physics tick ran line `applied`; check its pose
        int i = applied;
        double dx = p.getX() - plan.x[i], dy = p.getY() - plan.y[i], dz = p.getZ() - plan.z[i];
        double d = Math.sqrt(dx * dx + dy * dy + dz * dz);
        if (d != 0.0) {
            divergences++;
            if (firstDivergence < 0) firstDivergence = i;
            NetLog.playback("diverge", "line=" + i, "dx=" + dx, "dy=" + dy, "dz=" + dz, "d=" + d);
        }
        maxDev = Math.max(maxDev, d);
        if (moved || p.getYRot() != exactYaw || p.getXRot() != exactPitch) {
            endAttempt(false, "rotation changed outside playback at line " + i, i, d);
            return;
        }
        if (d > plan.tol) {
            NetLog.playback("abort", "line=" + i, "dx=" + dx, "dy=" + dy, "dz=" + dz, "d=" + d, "tol=" + plan.tol);
            endAttempt(false, String.format("off the plan by %.3g blocks after line %d (tolerance %.3g)", d, i, plan.tol), i, d);
            return;
        }
        if (finishLine >= 0 && i >= finishLine + POST_FINISH) {
            endAttempt(true, "complete", -1, 0.0);
            return;
        }
        if (i >= plan.lines) {
            if (finishLine >= 0) endAttempt(true, "complete", -1, 0.0);
            else endAttempt(false, "plan ended without \"Course complete!\"", -1, 0.0);
            return;
        }
        int line = i + 1;
        int react = -1;
        if (pendingGlideOff) {
            pendingGlideOff = false;
            glideOffs.add(line);
            for (int k = 0; k < plan.reactions.size(); k++) {
                Plan.React r = plan.reactions.get(k);
                if (!fired[k] && r.event().equals("glide_off") && r.t0() <= line && line <= r.t1()) {
                    fired[k] = true;
                    react = k;
                    break;
                }
            }
            NetLog.playback("glide_off", "line=" + line, "react=" + (react < 0 ? "-" : Integer.toString(react)));
            if (react >= 0) {
                reactions.add(line);
                reactionDirectives.add(react);
                NetLog.playback("react", "line=" + line, "directive=" + react, "event=glide_off", "action=press_jump");
            }
        }
        apply(line, react >= 0);
    }

    /**
     * Sets line {@code line}'s rotation and keys for the physics right after. A reaction's
     * press_jump is a jump edge on this tick; the jump then counts as held, so a J on the
     * next line is a hold, not another press.
     */
    private static void apply(int line, boolean pressJump) {
        setRot(plan.yaw[line], plan.pitch[line]);
        exactYaw = shownYaw;
        exactPitch = shownPitch;
        applied = line;
        int k = plan.keys[line];
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
        fired = new boolean[plan.reactions.size()];
        reactions = new ArrayList<>();
        reactionDirectives = new ArrayList<>();
        glideOffs = new ArrayList<>();
        pendingGlideOff = false;
        NetLog.playback("attempt", "n=" + attempt);
        useResetItem(mc);
    }

    private static void endAttempt(boolean ok, String reason, int abortLine, double abortDev) {
        camera = false;
        setKeys(0, false);
        jumpDown = false;
        HISTORY.add(new Attempt(attempt, ok, reason, applied, divergences, maxDev, firstDivergence, abortLine, abortDev,
                List.copyOf(reactions), List.copyOf(reactionDirectives), List.copyOf(glideOffs), finishLine));
        NetLog.playback("end", "attempt=" + attempt, "result=" + (ok ? "ok" : "fail"), "reason=" + reason,
                "line=" + applied, "divergences=" + divergences, "max_dev=" + maxDev, "reactions=" + reactions.size(),
                "finish_line=" + finishLine);
        if (ok) {
            done++;
            fails = 0;
            say(String.format("run %d ok (%s), %d divergent ticks, max %.2e, %d reactions", done, finishText, divergences,
                    maxDev, reactions.size()));
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
        return p.getX() == plan.x[0] && p.getY() == plan.y[0] && p.getZ() == plan.z[0] && !p.isFallFlying();
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
