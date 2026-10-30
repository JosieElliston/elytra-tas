package jealoustone.elytratas.phase;

import io.netty.channel.Channel;
import jealoustone.elytratas.ElytraTasClient;
import jealoustone.elytratas.mixin.ConnectionAccessor;
import jealoustone.elytratas.mixin.DeltaTrackerTimerAccessor;
import net.minecraft.client.Minecraft;
import net.minecraft.client.multiplayer.ClientPacketListener;
import net.minecraft.network.Connection;
import net.minecraft.network.PacketListener;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.game.ClientboundCommandSuggestionsPacket;
import net.minecraft.network.protocol.game.ClientboundSetHealthPacket;
import net.minecraft.network.protocol.game.ServerboundCommandSuggestionPacket;
import net.minecraft.network.protocol.game.ServerboundMovePlayerPacket;
import net.minecraft.network.protocol.ping.ClientboundPongResponsePacket;
import net.minecraft.network.protocol.ping.ServerboundPingRequestPacket;
import net.minecraft.util.Util;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.IdentityHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ThreadLocalRandom;
import java.util.concurrent.TimeUnit;

/**
 * Tick phase between this client and the server, for {@code /etas phase}. Runs whether or
 * not the network log is on.
 *
 * The server's marker is ClientboundSetHealthPacket: vanilla sends it from
 * ServerPlayer.doTick, but only when health or food changed. EMC sends it every tick.
 * It gives the client wait and the tick interval. The server flushes it at the end of the
 * tick (tickChildren suspends flushing until every level and player has ticked), so
 * measured from a move it is server wait plus that tick's work time.
 *
 * The tick start comes from probes: tab-completion requests sent at random times. The
 * server answers them on its main thread between ticks, so one arriving mid-tick waits
 * until the tick ends and one arriving between ticks is answered at once. Against the send
 * time's phase on the client's tick grid, the extra delay is a sawtooth: it jumps up by the
 * tick's work time W where the tick starts and falls back to 0 where it ends. Fitting that
 * gives the tick start (as a send phase) and W. Moves cross the same edge, so the server
 * wait for moves is the tick start minus the phase moves are sent at.
 *
 * Times are System.nanoTime(). Socket-level times (move, ping and probe writes; set_health,
 * pong and probe-reply decodes) give the round trip and the probe delays; the time a packet
 * reaches the listener (after any delaying pipeline handler) gives the client wait.
 *
 * Pings and probes go out only for user@server pairs on the whitelist (LoggerConfig); elsewhere the
 * tracker only observes, and the rows that need them stay empty.
 *
 * Threads: the encode/wire/decode/listener hooks run on the connection's Netty event loop,
 * ticks and the command on the client thread. Samples are guarded by the class lock.
 */
public final class PhaseTracker {
    public static final long TICK = 50_000_000L;
    static final long KEEP = 120_000_000_000L;
    private static final int PING_INTERVAL_TICKS = 5;
    /** Delay between probes, uniform, so their send phases cover the tick evenly. 4 a second on average. */
    private static final int PROBE_MIN_MS = 125, PROBE_MAX_MS = 375;
    /** The least time the probe fit looks back over (unless cut). why? see fitSawtooth. */
    private static final long PROBE_WINDOW = 20_000_000_000L;
    /**
     * What probes ask completions for: tried in turn until one gets replies, then kept.
     * why these? each always has suggestions (online players, help topics, the commands you
     * can run). EMC sent no reply at all to 1113 probes for "/etas", which has none
     * (2026-10-29 16:02–16:08 logs). The vanilla server replies even when empty, so
     * presumably EMC's server software or proxy drops empty replies; unconfirmed. A proxy or plugin that answers
     * completions off the server's main thread gives replies with no sawtooth; the fit then
     * reports nothing.
     */
    private static final String[] PROBE_TEXTS = {"/msg ", "/tell ", "/help ", "/"};

    private static volatile Connection connection;
    private static volatile Channel channel;
    /** The server's address as typed, or null in singleplayer. */
    private static volatile String server;
    private static volatile String user;

    // guarded by PhaseTracker.class
    private static final Series rtt = new Series();
    /** (set_health decode - latest move write) mod TICK; minus the round trip, it's server wait + work. */
    private static final Series upRaw = new Series();
    /** Move write phase on the client's tick grid. */
    private static final Series movePhase = new Series();
    /** Move write minus the start of the client tick that sent it. */
    private static final Series clientWork = new Series();
    /** Probe replies: {decode time, send phase on the grid, reply time minus send time}. */
    private static final ArrayDeque<long[]> probeSamples = new ArrayDeque<>();
    private static final Series down = new Series();
    /** set_health decode times; the value is unused. */
    private static final Series marker = new Series();
    private static final Jump jump = new Jump();
    /** Phase of the client's tick grid (DeltaTracker), in [0, TICK); -1 before the first tick. */
    private static volatile long gridPhase = -1;
    private static long cutAt = Long.MIN_VALUE;
    private static String cutReason = "";
    private static long lastMoveWrite = Long.MIN_VALUE;
    /** Ping time field (Util.getMillis) -> socket write time, or 0 while still queued. */
    private static final Map<Long, Long> pings = new HashMap<>();
    /**
     * Probe id -> {socket write time or 0 while still queued, index into PROBE_TEXTS}. Ids
     * are negative: the vanilla client's are positive, so it ignores our replies. Not -1:
     * vanilla's pending id starts at -1 before the first tab completion, so a reply with id
     * -1 completes its null future and the client disconnects (NPE in
     * completeCustomSuggestions; it did in a fresh singleplayer world, 2026-10-30).
     */
    private static final Map<Integer, long[]> probes = new HashMap<>();
    private static final int FIRST_PROBE_ID = -2;
    private static int nextProbeId = FIRST_PROBE_ID;
    /** The PROBE_TEXTS entry that got a reply, or -1 while still trying them in turn. */
    private static int probeText = -1;
    private static int probeTurn;
    private static int probesSent, probeReplies;
    /** When the latest client tick started. */
    private static long lastTickStart = Long.MIN_VALUE;
    private static final Map<Packet<?>, Long> reached = new IdentityHashMap<>();
    /** set_health handled on the client thread, waiting for the tick that first sees it. */
    private static final List<Long> awaitingTick = new ArrayList<>();
    private static int ticks;

    // Netty-thread-only: what the next wire write is (a channel never changes event loop)
    private static int pendingKind;
    private static long pendingPing;
    private static int pendingProbe;
    private static final int MOVE = 1, PING = 2, PROBE = 3;

    private PhaseTracker() {
    }

    // --- connection -------------------------------------------------------------------

    /**
     * ClientPlayConnectionEvents.INIT. It runs again on the same Connection after a
     * server-driven reconfiguration, which is how a proxy switches backends.
     */
    public static synchronized void init(Connection conn, String userName, String address) {
        user = userName;
        server = address;
        // A reconfiguration may mean a different backend, which may answer completions differently.
        probeText = -1;
        probesSent = 0;
        probeReplies = 0;
        if (conn == connection) {
            cut(System.nanoTime(), "reconfigured");
            return;
        }
        connection = conn;
        channel = ((ConnectionAccessor) conn).elytraTas$channel();
        rtt.clear();
        upRaw.clear();
        movePhase.clear();
        clientWork.clear();
        probeSamples.clear();
        probes.clear();
        down.clear();
        marker.clear();
        jump.reset();
        pings.clear();
        reached.clear();
        awaitingTick.clear();
        lastMoveWrite = Long.MIN_VALUE;
        cutAt = System.nanoTime();
        cutReason = "connected";
        PhaseCommand.forgetLast();
        scheduleProbe(channel);
    }

    /** Whether pings and probes go out: connected, with this user and server whitelisted. Read live, so a config change applies at once. */
    static boolean sending() {
        return connection != null && ElytraTasClient.CONFIG.whitelisted(user, server);
    }

    static String server() {
        return server;
    }

    static String user() {
        return user;
    }

    public static synchronized void disconnect() {
        connection = null;
        channel = null;
    }

    private static void cut(long at, String reason) {
        cutAt = at;
        cutReason = reason;
        jump.reset();
    }

    // --- client thread ----------------------------------------------------------------

    /**
     * START_CLIENT_TICK: everything handled since the last tick is first seen by this one.
     * Also reads the tick grid: the timer's residual says how far past its latest tick
     * point {@code lastMs} is. Util.getMillis is System.nanoTime / 1e6, so the grid is in
     * nanoTime too. why? the grid holds for the whole process (after a hitch the timer drops
     * ticks but keeps the residual), while tick starts trail it by up to a frame.
     */
    public static synchronized void tickStart(Minecraft client) {
        long now = System.nanoTime();
        lastTickStart = now;
        if (client.getDeltaTracker() instanceof DeltaTrackerTimerAccessor t) {
            long grid = t.elytraTas$lastMs() * 1_000_000L - Math.round(t.elytraTas$deltaTickResidual() * (double) TICK);
            gridPhase = Math.floorMod(grid, TICK);
        }
        if (awaitingTick.isEmpty()) return;
        for (long r : awaitingTick) down.add(now, Math.floorMod(now - r, TICK));
        awaitingTick.clear();
    }

    /** END_CLIENT_TICK: send our own pings. why? vanilla only pings with the F3 network chart open. */
    public static void tickEnd(Minecraft client) {
        Connection conn = connection;
        ClientPacketListener listener = client.getConnection();
        if (conn == null || listener == null || listener.getConnection() != conn || !sending()) return;
        if (++ticks % PING_INTERVAL_TICKS != 0) return;
        // Util.getMillis, as vanilla does, so the F3 chart reads our pongs correctly too.
        long time = Util.getMillis();
        synchronized (PhaseTracker.class) {
            if (pings.containsKey(time)) return;
            pings.keySet().removeIf(t -> t < time - 5_000);
            pings.put(time, 0L);
        }
        conn.send(new ServerboundPingRequestPacket(time));
    }

    // --- probes (Netty thread) -------------------------------------------------------

    /**
     * Probes go out from the connection's event loop at random times. why not the client
     * thread? it only runs tasks at frame starts, and at a steady frame rate those hit a
     * few fixed phases of the tick grid, which wouldn't sweep the tick.
     */
    private static void scheduleProbe(Channel ch) {
        if (ch == null) return;
        long delay = ThreadLocalRandom.current().nextLong(PROBE_MIN_MS, PROBE_MAX_MS + 1);
        ch.eventLoop().schedule(() -> probe(ch), delay, TimeUnit.MILLISECONDS);
    }

    private static void probe(Channel ch) {
        Connection conn = connection;
        if (ch != channel || conn == null || !conn.isConnected()) return;
        // Only while in play: during a reconfiguration the outbound protocol has no such packet.
        if (conn.getPacketListener() instanceof ClientPacketListener && sending()) {
            int id, text;
            synchronized (PhaseTracker.class) {
                id = nextProbeId;
                nextProbeId = nextProbeId <= -1_000_000_000 ? FIRST_PROBE_ID : nextProbeId - 1;
                text = probeText >= 0 ? probeText : probeTurn++ % PROBE_TEXTS.length;
                if (probes.size() > 100) probes.clear();
                probes.put(id, new long[]{0, text});
                probesSent++;
            }
            conn.send(new ServerboundCommandSuggestionPacket(id, PROBE_TEXTS[text]));
        }
        scheduleProbe(ch);
    }

    /** PacketProcessor ran a queued packet on the client thread. */
    public static void mainHandle(PacketListener listener, Packet<?> packet) {
        if (!(packet instanceof ClientboundSetHealthPacket) || !ours(listener)) return;
        synchronized (PhaseTracker.class) {
            Long r = reached.remove(packet);
            if (r != null) awaitingTick.add(r);
        }
    }

    // --- Netty thread -----------------------------------------------------------------

    /** PacketEncoder.encode returned; the next wire write on this channel is this packet. */
    public static void encoded(Channel ch, Packet<?> packet) {
        if (ch != channel) return;
        pendingKind = 0;
        if (packet instanceof ServerboundMovePlayerPacket) {
            pendingKind = MOVE;
        } else if (packet instanceof ServerboundPingRequestPacket p) {
            synchronized (PhaseTracker.class) {
                if (!pings.containsKey(p.getTime())) return;
            }
            pendingKind = PING;
            pendingPing = p.getTime();
        } else if (packet instanceof ServerboundCommandSuggestionPacket p && p.getId() < 0) {
            pendingKind = PROBE;
            pendingProbe = p.getId();
        }
    }

    /** Wire tap write: the frame reached the socket end of the pipeline. */
    public static void wireWrite(Channel ch) {
        if (ch != channel || pendingKind == 0) return;
        long now = System.nanoTime();
        int kind = pendingKind;
        pendingKind = 0;
        synchronized (PhaseTracker.class) {
            switch (kind) {
                case MOVE -> {
                    lastMoveWrite = now;
                    long g = gridPhase;
                    if (g >= 0) movePhase.add(now, Math.floorMod(now - g, TICK));
                    long work = now - lastTickStart;
                    if (lastTickStart != Long.MIN_VALUE && work >= 0 && work < TICK) clientWork.add(now, work);
                }
                case PING -> {
                    if (pings.containsKey(pendingPing)) pings.put(pendingPing, now);
                }
                case PROBE -> {
                    long[] p = probes.get(pendingProbe);
                    if (p != null) p[0] = now;
                }
                default -> {
                }
            }
        }
    }

    /** PacketDecoder.decode produced {@code packet}; this is within microseconds of the socket read. */
    public static void decoded(Channel ch, Packet<?> packet) {
        if (ch != channel) return;
        long now = System.nanoTime();
        switch (packet) {
            case ClientboundSetHealthPacket p -> {
                synchronized (PhaseTracker.class) {
                    marker.add(now, 0);
                    if (lastMoveWrite != Long.MIN_VALUE && now - lastMoveWrite < 1_000_000_000L) {
                        upRaw.add(now, Math.floorMod(now - lastMoveWrite, TICK));
                    }
                    long g = gridPhase;
                    if (g >= 0) {
                        double step = jump.add(now, Math.floorMod(now - g, TICK));
                        if (!Double.isNaN(step)) cut(jump.stepAt, String.format("phase jump %+.0f ms", step / 1e6));
                    }
                }
            }
            case ClientboundPongResponsePacket p -> {
                synchronized (PhaseTracker.class) {
                    Long w = pings.remove(p.time());
                    if (w != null && w != 0L) rtt.add(now, now - w);
                }
            }
            case ClientboundCommandSuggestionsPacket p when p.id() < 0 -> {
                synchronized (PhaseTracker.class) {
                    long[] sent = probes.remove(p.id());
                    long g = gridPhase;
                    if (sent == null || sent[0] == 0L || g < 0) return;
                    probeReplies++;
                    if (probeText < 0) probeText = (int) sent[1];
                    long w = sent[0];
                    probeSamples.addLast(new long[]{now, Math.floorMod(w - g, TICK), now - w});
                    while (probeSamples.peekFirst()[0] < now - KEEP) probeSamples.removeFirst();
                }
            }
            default -> {
            }
        }
    }

    /** Connection.genericsFtw: the packet left the pipeline and reached its listener. */
    public static void reachedListener(Packet<?> packet, PacketListener listener) {
        if (!(packet instanceof ClientboundSetHealthPacket) || !ours(listener)) return;
        long now = System.nanoTime();
        synchronized (PhaseTracker.class) {
            if (reached.size() > 1_000) reached.clear();
            reached.put(packet, now);
        }
    }

    private static boolean ours(PacketListener listener) {
        Connection conn = connection;
        return conn != null && listener instanceof ClientPacketListener l && l.getConnection() == conn;
    }

    // --- snapshot -------------------------------------------------------------------

    /**
     * Window statistics, in nanoseconds; NaN where there is no data. The loop from a client
     * tick start through a move, its reply and the next client tick that sees the reply is
     * round trip + server wait + server work + client wait + client work, a whole number of
     * ticks. clientWaitDerived says client wait came from that identity, not a per-tick packet.
     */
    record Snapshot(long at, long from, String cutReason, int rttSamples, double rtt, double rttSpread,
                    int probesSent, int probeReplies, String probeText, int probes, double serverWait, double serverWork, boolean perTick, double clientWait,
                    boolean clientWaitDerived, double clientWork, double waitPlusWork, double tickInterval,
                    double longest) {
    }

    static synchronized Snapshot snapshot(long windowNanos) {
        long now = System.nanoTime();
        long from = Math.max(now - windowNanos, cutAt);
        String reason = from == cutAt ? cutReason : "";

        double[] r = rtt.values(from);
        Arrays.sort(r);
        double rttMed = quantile(r, 0.5);
        double spread = (quantile(r, 0.9) - quantile(r, 0.1)) / 2;

        // The tick start holds steady, so the probes get at least PROBE_WINDOW whatever the display window.
        long probeFrom = Math.max(Math.min(from, now - PROBE_WINDOW), cutAt);
        List<long[]> ps = probeSamples.stream().filter(x -> x[0] >= probeFrom).toList();
        double[] pr = rtt.values(probeFrom);
        Arrays.sort(pr);
        double base = quantile(pr, 0.25);
        double[] fit = Double.isNaN(base) ? null : fitSawtooth(ps, base);
        double moveAt = circularMean(movePhase.values(from));
        double serverWait = Double.NaN, serverWork = Double.NaN;
        if (fit != null) {
            serverWork = fit[1];
            if (!Double.isNaN(moveAt)) serverWait = Math.floorMod(Math.round(fit[0] - moveAt), TICK);
        }
        double[] cw = clientWork.values(from);
        Arrays.sort(cw);
        double work = quantile(cw, 0.5);

        long[] m = marker.times(from);
        double span = now - from;
        boolean perTick = m.length >= 2 && m.length >= 0.5 * span / TICK;
        double waitPlusWork = Double.NaN, dn = Double.NaN, interval = Double.NaN, longest = Double.NaN;
        if (perTick) {
            double raw = circularMean(upRaw.values(from));
            if (!Double.isNaN(raw) && !Double.isNaN(rttMed)) waitPlusWork = Math.floorMod(Math.round(raw - rttMed), TICK);
            dn = circularMean(down.values(from));
            interval = (double) (m[m.length - 1] - m[0]) / (m.length - 1);
            longest = 0;
            for (int i = 1; i < m.length; i++) longest = Math.max(longest, m[i] - m[i - 1]);
        }
        boolean derived = false;
        if (Double.isNaN(dn)) {
            double rest = rttMed + serverWait + serverWork + work;
            if (!Double.isNaN(rest)) {
                dn = Math.floorMod(-Math.round(rest), TICK);
                derived = true;
            }
        }
        return new Snapshot(now, from, reason, r.length, rttMed, spread, probesSent, probeReplies,
                probeText >= 0 ? PROBE_TEXTS[probeText] : null, ps.size(), serverWait, serverWork, perTick,
                dn, derived, work, waitPlusWork, interval, longest);
    }

    /**
     * Fits the probes' extra delay (reply time minus send time minus a low round trip)
     * against their send phase with a sawtooth: W - x for x = (phase - start) mod TICK below
     * W, else 0. Returns {start, W}, or null when the fit doesn't look like a sawtooth.
     *
     * The loss is pinball at TAU = 0.2, which fits the lower edge. why? EMC sometimes holds
     * a reply until a later flush: about 30% of pongs and probe replies came 10–90 ms late,
     * arriving with the next tick's set_health and chunk packets (2026-10-29 16:16 log).
     * Those only ever add delay, and a symmetric L1 fit was pulled off the edge by them.
     *
     * The check looks at the shape: W of at least 8 ms; the 25th percentile of probes in
     * the first half of the busy part at least W/2; that of probes in the idle part at most
     * 0.15 W. why these? replayed over that log in sliding 20 s windows (about 80 probes),
     * 89 of 89 fits passed and 87 had the start within 3 ms of the 25.5 ms the whole log
     * gives. Simulated with 30% held replies: 1 in 80 passed with no jump at all; W = 10
     * passed 50 in 80; W = 5 rarely. A loss-ratio check passed fewer than half of the real
     * windows.
     */
    static double[] fitSawtooth(List<long[]> samples, double rttNanos) {
        int n = samples.size();
        if (n < 20) return null;
        double[] phase = new double[n], extra = new double[n];
        for (int i = 0; i < n; i++) {
            phase[i] = samples.get(i)[1] / 1e6;
            extra[i] = (samples.get(i)[2] - rttNanos) / 1e6;
        }
        double tick = TICK / 1e6;
        double bestL = Double.MAX_VALUE, bestS = 0, bestW = 0;
        for (double s = 0; s < tick; s += 1) {
            for (double w = 0; w <= 45; w += 1) {
                double l = sawtoothLoss(phase, extra, s, w, tick);
                if (l < bestL) {
                    bestL = l;
                    bestS = s;
                    bestW = w;
                }
            }
        }
        double s0 = bestS, w0 = bestW;
        for (double s = s0 - 1; s <= s0 + 1; s += 0.25) {
            for (double w = Math.max(0, w0 - 1); w <= w0 + 1; w += 0.25) {
                double l = sawtoothLoss(phase, extra, s, w, tick);
                if (l < bestL) {
                    bestL = l;
                    bestS = s;
                    bestW = w;
                }
            }
        }
        if (bestW < 8 || bestW > 42) return null;
        List<Double> busy = new ArrayList<>(), idle = new ArrayList<>();
        for (int i = 0; i < n; i++) {
            double x = ((phase[i] - bestS) % tick + tick) % tick;
            if (x < bestW / 2) busy.add(extra[i]);
            else if (x >= bestW) idle.add(extra[i]);
        }
        if (busy.size() < 4 || idle.size() < 4) return null;
        if (lowQuartile(busy) < bestW / 2 || lowQuartile(idle) > 0.15 * bestW) return null;
        return new double[]{Math.floorMod(Math.round(bestS * 1e6), TICK), bestW * 1e6};
    }

    private static final double TAU = 0.2;

    private static double lowQuartile(List<Double> xs) {
        double[] a = xs.stream().mapToDouble(Double::doubleValue).toArray();
        Arrays.sort(a);
        return quantile(a, 0.25);
    }

    private static double sawtoothLoss(double[] phase, double[] extra, double start, double work, double tick) {
        double l = 0;
        for (int i = 0; i < phase.length; i++) {
            double x = ((phase[i] - start) % tick + tick) % tick;
            double r = extra[i] - (x < work ? work - x : 0);
            l += r >= 0 ? TAU * r : (TAU - 1) * r;
        }
        return l;
    }

    static double quantile(double[] sorted, double q) {
        if (sorted.length == 0) return Double.NaN;
        return sorted[(int) Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
    }

    /** Mean of values on a TICK-long circle, in [0, TICK). why? a phase near 0/50 ms wraps, and a plain mean of 1 and 49 is 25. */
    static double circularMean(double[] vs) {
        if (vs.length == 0) return Double.NaN;
        double s = 0, c = 0;
        for (double v : vs) {
            double a = 2 * Math.PI * v / TICK;
            s += Math.sin(a);
            c += Math.cos(a);
        }
        double a = Math.atan2(s, c);
        return Math.floorMod(Math.round(a / (2 * Math.PI) * TICK), TICK);
    }

    /** A difference taken on a TICK-long circle, in [-TICK/2, TICK/2). */
    static double wrap(double d) {
        double x = (d % TICK + TICK + TICK / 2.0) % TICK;
        return x - TICK / 2.0;
    }

    /** Timestamped samples, oldest first, kept for {@link #KEEP}. */
    private static final class Series {
        private final ArrayDeque<long[]> q = new ArrayDeque<>();

        void add(long t, long v) {
            q.addLast(new long[]{t, v});
            while (!q.isEmpty() && q.peekFirst()[0] < t - KEEP) q.removeFirst();
        }

        void clear() {
            q.clear();
        }

        double[] values(long from) {
            return q.stream().filter(s -> s[0] >= from).mapToDouble(s -> s[1]).toArray();
        }

        long[] times(long from) {
            return q.stream().filter(s -> s[0] >= from).mapToLong(s -> s[0]).toArray();
        }
    }

    /**
     * Step detector on the server's tick phase against the client's tick grid
     * (set_health decode time - grid, mod TICK). That isolates server and network: frame
     * timing doesn't enter. Every {@link #CHECK} samples it compares the circular means of
     * the last two blocks of {@link #BLOCK} samples (5 s each); a difference over
     * {@link #STEP} is a jump, placed at the split of the last two blocks that best
     * separates them. why these sizes? on EMC (2026-10-28 logs) the phase wanders about
     * ±4 ms second to second and drifts about 1 ms a minute. Replayed over three logs (~23
     * minutes): medians of 20 samples cut every second or two, 5 s blocks at 6 ms cut 5
     * times, at 8 ms once, at a real settle 9 s after joining.
     */
    private static final class Jump {
        static final int BLOCK = 100, CHECK = 20;
        static final double STEP = 8e6;
        final long[] times = new long[2 * BLOCK];
        final double[] values = new double[2 * BLOCK];
        int n;
        int checks;
        long stepAt;

        void reset() {
            n = 0;
            checks = 0;
        }

        /** The step size when one is detected, else NaN. */
        double add(long t, double v) {
            if (n == 2 * BLOCK) {
                System.arraycopy(times, 1, times, 0, n - 1);
                System.arraycopy(values, 1, values, 0, n - 1);
                n--;
            }
            times[n] = t;
            values[n] = v;
            n++;
            if (n < 2 * BLOCK || ++checks % CHECK != 0) return Double.NaN;
            double d = wrap(mean(BLOCK, 2 * BLOCK) - mean(0, BLOCK));
            if (Math.abs(d) <= STEP) return Double.NaN;
            int best = BLOCK;
            double bestD = 0;
            for (int split = CHECK; split <= 2 * BLOCK - CHECK; split += 10) {
                double x = Math.abs(wrap(mean(split, 2 * BLOCK) - mean(0, split)));
                if (x > bestD) {
                    bestD = x;
                    best = split;
                }
            }
            stepAt = times[best];
            return d;
        }

        private double mean(int from, int to) {
            return circularMean(Arrays.copyOfRange(values, from, to));
        }
    }
}
