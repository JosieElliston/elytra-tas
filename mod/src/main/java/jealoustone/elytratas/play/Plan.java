package jealoustone.elytratas.play;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.EnumSet;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * A plan of solver/PLAN-FORMAT.md version 2 or 1, or a plain {@code .tas} tape (format in
 * extract_run.py's docstring): a root segment and named ones, each with its tick lines
 * and directives, checked as the spec's "Checks" says. Ticks are the plan's numbering:
 * line {@code t} is tick {@code t} after the start, in whichever segment flies it.
 */
public final class Plan {
    public static final String FORMAT = "etas-plan 2";
    public static final String FORMAT_1 = "etas-plan 1";
    public static final double DEFAULT_TOL = 1e-4;
    public static final String ROOT = "root";
    /** Header keys only a plan has; a plain tape with any of them is refused. */
    private static final Set<String> PLAN_KEYS = Set.of("account", "course", "course_sha256", "world_sha256", "diverge_tol",
            "velocity_tol");

    /** Input bits, as the logger's {@code T} records and the oracle's Driver use them. */
    public static final int W = 1, S = 2, A = 4, D = 8, J = 16, SPRINT = 64;

    /** What the server did, as playback observes it before a line. */
    public enum Event {
        GLIDE_OFF("glide_off"), SET_VELOCITY("set_velocity");

        public final String id;

        Event(String id) {
            this.id = id;
        }

        static Event of(String s) {
            for (Event e : values()) if (e.id.equals(s)) return e;
            return null;
        }
    }

    /** {@code @react}; {@code index} is its place among all the plan's reactions, in file order. */
    public record React(int t0, int t1, String event, String action, int fileLine, int index) {
    }

    /** {@code @branch}; {@code v} is null for glide_off. */
    public record Branch(int tick, Event event, double[] v, String child, int fileLine) {
        public String describe() {
            return event.id + (v == null ? "" : " " + v[0] + "," + v[1] + "," + v[2]) + " -> " + child;
        }
    }

    public record Ignore(int t0, int t1, Event event, int fileLine) {
    }

    public static final class PlanException extends Exception {
        PlanException(String message) {
            super(message);
        }
    }

    /** A segment: tick lines {@code start .. end() - 1} and its directives. */
    public static final class Segment {
        public final String name;
        public final int start;
        final int fileLine;
        final List<String[]> rawTicks = new ArrayList<>();
        final List<Integer> tickAt = new ArrayList<>();
        final List<String[]> rawDirectives = new ArrayList<>();
        final List<Integer> directiveAt = new ArrayList<>();
        public float[] yaw, pitch;
        public int[] keys;
        public double[] x, y, z;
        public final List<React> reactions = new ArrayList<>();
        public final List<Branch> branches = new ArrayList<>();
        public final List<Ignore> ignores = new ArrayList<>();
        public String gotoName;
        public Segment next;
        final Map<Branch, Segment> children = new HashMap<>();

        Segment(String name, int start, int fileLine) {
            this.name = name;
            this.start = start;
            this.fileLine = fileLine;
        }

        public int lines() {
            return yaw.length;
        }

        /** The tick after the last line: where its goto target starts and the last tick its branches may name. */
        public int end() {
            return start + yaw.length;
        }

        public boolean has(int t) {
            return t >= start && t < end();
        }

        public int i(int t) {
            return t - start;
        }

        public Segment child(Branch b) {
            return children.get(b);
        }

        boolean branchesAt(int t) {
            for (Branch b : branches) if (b.tick == t) return true;
            return false;
        }
    }

    public final String name;
    public final Map<String, String> header;
    /** True for any {@code format=}, false for a plain tape. */
    public final boolean versioned;
    /** 2 for {@code etas-plan 2}, 1 for version 1 and plain tapes. */
    public final int version;
    /** The start pose and rotation: line 0. */
    public final double x0, y0, z0;
    public final float yaw0, pitch0;
    public final double tol;
    /** {@code velocity_tol=}, or NaN when absent (then the plan has no set_velocity branch). */
    public final double velocityTol;
    public final Segment root;
    /** Every segment, the root first, then file order. */
    public final Map<String, Segment> segments;
    /** Every reaction of every segment, file order. */
    public final List<React> reactions;
    /** Every tick line in the file. */
    public final int tickLines;
    /** The last tick any segment has. */
    public final int lastTick;
    public final int branchCount;

    private Plan(String name, Map<String, String> header, boolean versioned, int version, double[] start, float yaw0,
                 float pitch0, double tol, double velocityTol, Map<String, Segment> segments, List<React> reactions) {
        this.name = name;
        this.header = header;
        this.versioned = versioned;
        this.version = version;
        this.x0 = start[0];
        this.y0 = start[1];
        this.z0 = start[2];
        this.yaw0 = yaw0;
        this.pitch0 = pitch0;
        this.tol = tol;
        this.velocityTol = velocityTol;
        this.segments = segments;
        this.root = segments.get(ROOT);
        this.reactions = reactions;
        int n = 0, last = 0, b = 0;
        for (Segment s : segments.values()) {
            n += s.lines();
            last = Math.max(last, s.end() - 1);
            b += s.branches.size();
        }
        this.tickLines = n;
        this.lastTick = last;
        this.branchCount = b;
    }

    /** Header value or null. */
    public String get(String key) {
        return header.get(key);
    }

    public String format() {
        return !versioned ? "tas" : version == 2 ? FORMAT : FORMAT_1;
    }

    public static Plan read(Path file, String name) throws PlanException {
        List<String> raw;
        try {
            raw = Files.readAllLines(file);
        } catch (IOException e) {
            throw new PlanException("can't read " + file + ": " + e.getMessage());
        }
        return parse(raw, name);
    }

    public static Plan parse(List<String> raw, String name) throws PlanException {
        Map<String, String> header = new LinkedHashMap<>();
        Map<String, Segment> segs = new LinkedHashMap<>();
        Segment cur = new Segment(ROOT, 1, 0);
        segs.put(ROOT, cur);
        for (int n = 0; n < raw.size(); n++) {
            String line = raw.get(n);
            int hash = line.indexOf('#');
            if (hash >= 0) line = line.substring(0, hash);
            line = line.strip();
            if (line.isEmpty()) continue;
            String[] f = line.split("\\s+");
            String at = "line " + (n + 1);
            if (f[0].equals("@segment")) {
                if (f.length != 3) throw new PlanException(at + ": want @segment name start");
                if (!f[1].matches("[A-Za-z0-9_.+-]+") || f[1].equals(ROOT)) {
                    throw new PlanException(at + ": segment name " + f[1] + ": want [A-Za-z0-9_.+-]+, not " + ROOT);
                }
                if (segs.containsKey(f[1])) throw new PlanException(at + ": segment " + f[1] + " is defined twice");
                int start = integer(f[2], at);
                if (start < 1) throw new PlanException(at + ": start " + start + ": lines are ticks 1 onward");
                cur = new Segment(f[1], start, n + 1);
                segs.put(f[1], cur);
            } else if (line.startsWith("@")) {
                cur.rawDirectives.add(f);
                cur.directiveAt.add(n + 1);
            } else if (line.contains("=")) {
                if (!cur.name.equals(ROOT)) throw new PlanException(at + ": a header line inside segment " + cur.name);
                int eq = line.indexOf('=');
                header.put(line.substring(0, eq).strip(), line.substring(eq + 1).strip());
            } else {
                cur.rawTicks.add(f);
                cur.tickAt.add(n + 1);
            }
        }

        String format = header.get("format");
        boolean versioned = format != null;
        int version;
        if (!versioned) version = 1;
        else if (format.equals(FORMAT)) version = 2;
        else if (format.equals(FORMAT_1)) version = 1;
        else throw new PlanException("format=" + format + ": this mod reads format=" + FORMAT + " and " + FORMAT_1);
        if (!versioned) {
            for (String k : header.keySet()) {
                if (PLAN_KEYS.contains(k)) throw new PlanException(k + "= is a plan key, but there is no format=");
            }
            for (Segment s : segs.values()) {
                if (!s.rawDirectives.isEmpty()) {
                    throw new PlanException("line " + s.directiveAt.getFirst() + ": an @ line, but there is no format=");
                }
            }
        }
        if (version == 1 && segs.size() > 1) {
            throw new PlanException("line " + segs.values().stream().skip(1).findFirst().get().fileLine
                    + ": @segment isn't in format " + FORMAT_1 + " (known: @react)");
        }

        String startKey = header.get("start");
        if (startKey == null) throw new PlanException("no start= (the start contract)");
        String[] s = startKey.split("\\s+");
        if (s.length < 5) throw new PlanException("start=" + startKey + ": want x y z yaw pitch as bits");
        double[] start = {dbits(s[0], "start"), dbits(s[1], "start"), dbits(s[2], "start")};
        float yaw0 = fbits(s[3], "start"), pitch0 = fbits(s[4], "start");

        double tol = tolerance(header, "diverge_tol", DEFAULT_TOL);
        double vtol = tolerance(header, "velocity_tol", Double.NaN);

        int total = 0;
        for (Segment g : segs.values()) {
            int count = g.rawTicks.size();
            total += count;
            g.yaw = new float[count];
            g.pitch = new float[count];
            g.keys = new int[count];
            g.x = new double[count];
            g.y = new double[count];
            g.z = new double[count];
            for (int i = 0; i < count; i++) {
                String[] f = g.rawTicks.get(i);
                String at = "line " + g.tickAt.get(i);
                if (f.length < 7) throw new PlanException(at + ": want tick yaw pitch keys x y z");
                int t = integer(f[0], at);
                if (t != g.start + i) throw new PlanException(at + ": tick " + t + ", expected " + (g.start + i));
                g.yaw[i] = fbits(f[1], at);
                g.pitch[i] = fbits(f[2], at);
                g.keys[i] = keys(f[3], at);
                g.x[i] = dbits(f[4], at);
                g.y[i] = dbits(f[5], at);
                g.z[i] = dbits(f[6], at);
            }
        }
        if (segs.get(ROOT).lines() == 0 && version == 1) throw new PlanException("no tick lines");

        String ticksKey = header.get("ticks");
        if (ticksKey != null && !ticksKey.equals(Integer.toString(total))) {
            throw new PlanException("header says ticks=" + ticksKey + ", the file has " + total + " tick lines");
        }

        List<React> reactions = new ArrayList<>();
        for (Segment g : segs.values()) directives(g, version, reactions);
        check(segs, vtol);
        return new Plan(name, header, versioned, version, start, yaw0, pitch0, tol, vtol, segs, List.copyOf(reactions));
    }

    private static void directives(Segment g, int version, List<React> reactions) throws PlanException {
        String known = version == 2 ? "@react, @branch, @goto, @ignore" : "@react";
        String fmt = version == 2 ? FORMAT : FORMAT_1;
        int lo = g.name.equals(ROOT) ? 1 : g.start + 1, end = g.end();
        for (int k = 0; k < g.rawDirectives.size(); k++) {
            String[] d = g.rawDirectives.get(k);
            int fileLine = g.directiveAt.get(k);
            String at = "line " + fileLine;
            boolean v2 = version == 2;
            switch (d[0]) {
                case "@react" -> {
                    if (d.length != 5) throw new PlanException(at + ": want @react t0 t1 event action");
                    int t0 = integer(d[1], at), t1 = integer(d[2], at);
                    if (t0 < lo || t1 < t0 || t1 >= end) {
                        throw new PlanException(at + ": window " + t0 + ".." + t1 + " isn't within ticks " + lo + ".." + (end - 1)
                                + (g.name.equals(ROOT) ? "" : " of segment " + g.name));
                    }
                    if (!d[3].equals("glide_off")) throw new PlanException(at + ": unknown event " + d[3] + " (known: glide_off)");
                    if (!d[4].equals("press_jump")) throw new PlanException(at + ": unknown action " + d[4] + " (known: press_jump)");
                    // press_jump has to be an edge on any tick of the window
                    for (int t = Math.max(t0, 2); t <= t1; t++) {
                        if ((g.keys[g.i(t - 1)] & J) != 0) {
                            throw new PlanException(at + ": press_jump at tick " + t + ", but tick " + (t - 1) + " holds J");
                        }
                    }
                    React r = new React(t0, t1, d[3], d[4], fileLine, reactions.size());
                    g.reactions.add(r);
                    reactions.add(r);
                }
                case "@branch" -> {
                    if (!v2) throw new PlanException(at + ": @branch isn't in format " + fmt + " (known: " + known + ")");
                    if (d.length < 4) throw new PlanException(at + ": want @branch tick event [payload] child");
                    int t = integer(d[1], at);
                    Event e = Event.of(d[2]);
                    if (e == null) throw new PlanException(at + ": unknown event " + d[2] + " (known: glide_off, set_velocity)");
                    double[] v = null;
                    if (e == Event.GLIDE_OFF && d.length != 4) throw new PlanException(at + ": want @branch tick glide_off child");
                    if (e == Event.SET_VELOCITY) {
                        if (d.length != 7) throw new PlanException(at + ": want @branch tick set_velocity vx vy vz child");
                        v = new double[]{dbits(d[3], at), dbits(d[4], at), dbits(d[5], at)};
                        for (double c : v) if (!Double.isFinite(c)) throw new PlanException(at + ": velocity isn't finite");
                    }
                    if (t < g.start || t > end) throw new PlanException(at + ": tick " + t + " isn't within " + g.start + ".." + end);
                    g.branches.add(new Branch(t, e, v, d[d.length - 1], fileLine));
                }
                case "@ignore" -> {
                    if (!v2) throw new PlanException(at + ": @ignore isn't in format " + fmt + " (known: " + known + ")");
                    if (d.length != 4) throw new PlanException(at + ": want @ignore t0 t1 event");
                    int t0 = integer(d[1], at), t1 = integer(d[2], at);
                    Event e = Event.of(d[3]);
                    if (e == null) throw new PlanException(at + ": unknown event " + d[3] + " (known: glide_off, set_velocity)");
                    if (t0 < g.start || t1 < t0 || t1 > end) {
                        throw new PlanException(at + ": window " + t0 + ".." + t1 + " isn't within " + g.start + ".." + end);
                    }
                    g.ignores.add(new Ignore(t0, t1, e, fileLine));
                }
                case "@goto" -> {
                    if (!v2) throw new PlanException(at + ": @goto isn't in format " + fmt + " (known: " + known + ")");
                    if (d.length != 2) throw new PlanException(at + ": want @goto segment");
                    if (g.gotoName != null) throw new PlanException(at + ": segment " + g.name + " has a second @goto");
                    g.gotoName = d[1];
                }
                default -> throw new PlanException(at + ": " + d[0] + " isn't in format " + fmt + " (known: " + known + ")");
            }
        }
    }

    /** The spec's checks across segments: targets, ambiguity, reactions against branches, DAG, reachability, consumed observations. */
    private static void check(Map<String, Segment> segs, double vtol) throws PlanException {
        for (Segment g : segs.values()) {
            String at = "segment " + g.name + ": ";
            for (int k = 0; k < g.branches.size(); k++) {
                Branch b = g.branches.get(k);
                Segment c = segs.get(b.child);
                String bl = "line " + b.fileLine + ": ";
                if (c == null) throw new PlanException(bl + "@branch to " + b.child + ": no such segment");
                if (c.start != b.tick) {
                    throw new PlanException(bl + "@branch " + b.tick + " to " + b.child + ": it starts at " + c.start);
                }
                g.children.put(b, c);
                if (b.event == Event.SET_VELOCITY && Double.isNaN(vtol)) {
                    throw new PlanException(bl + "@branch set_velocity, but no velocity_tol= in the header");
                }
                if (b.event == Event.GLIDE_OFF) {
                    for (React r : g.reactions) {
                        if (r.t0 <= b.tick && b.tick <= r.t1) {
                            throw new PlanException(bl + "@branch " + b.tick + " glide_off is inside the @react on line " + r.fileLine);
                        }
                    }
                }
                for (int q = 0; q < k; q++) {
                    Branch o = g.branches.get(q);
                    if (o.tick != b.tick || o.event != b.event) continue;
                    boolean clash = b.v == null;
                    if (b.v != null) {
                        clash = true;
                        for (int i = 0; i < 3; i++) clash &= Math.abs(o.v[i] - b.v[i]) <= 2 * vtol;
                    }
                    if (clash) {
                        throw new PlanException(bl + "@branch " + b.tick + " " + b.event.id + " to " + o.child + " and to " + b.child
                                + ": one observation would match both");
                    }
                }
            }
            if (g.gotoName != null) {
                Segment n = segs.get(g.gotoName);
                if (n == null) throw new PlanException(at + "@goto " + g.gotoName + ": no such segment");
                if (n.start != g.end()) {
                    throw new PlanException(at + "@goto " + g.gotoName + ": it starts at " + n.start + ", not at " + g.end());
                }
                g.next = n;
            }
        }
        // a DAG reachable from the root
        Map<Segment, Integer> state = new HashMap<>();
        dfs(segs.get(ROOT), state);
        for (Segment g : segs.values()) {
            if (!state.containsKey(g)) throw new PlanException("segment " + g.name + " is not reachable from the root");
        }
        // each observation is taken once along a chain of same-tick entries
        Map<Segment, EnumSet<Event>> consumed = new HashMap<>();
        for (boolean changed = true; changed; ) {
            changed = false;
            for (Segment g : segs.values()) {
                EnumSet<Event> here = consumed.getOrDefault(g, EnumSet.noneOf(Event.class));
                for (Branch b : g.branches) {
                    EnumSet<Event> set = b.tick == g.start ? EnumSet.copyOf(here) : EnumSet.noneOf(Event.class);
                    set.add(b.event);
                    changed |= consumed.computeIfAbsent(g.children.get(b), x -> EnumSet.noneOf(Event.class)).addAll(set);
                }
                if (g.next != null && g.end() == g.start && !here.isEmpty()) {
                    changed |= consumed.computeIfAbsent(g.next, x -> EnumSet.noneOf(Event.class)).addAll(here);
                }
            }
        }
        for (Segment g : segs.values()) {
            EnumSet<Event> set = consumed.get(g);
            if (set == null) continue;
            for (Branch b : g.branches) {
                if (b.tick == g.start && set.contains(b.event)) {
                    throw new PlanException("line " + b.fileLine + ": @branch " + b.tick + " " + b.event.id
                            + " can never match: entering segment " + g.name + " took that observation");
                }
            }
        }
    }

    private static void dfs(Segment g, Map<Segment, Integer> state) throws PlanException {
        Integer s = state.get(g);
        if (s != null) {
            if (s == 1) throw new PlanException("segment " + g.name + " is reachable from itself");
            return;
        }
        state.put(g, 1);
        for (Branch b : g.branches) dfs(g.children.get(b), state);
        if (g.next != null) dfs(g.next, state);
        state.put(g, 2);
    }

    private static double tolerance(Map<String, String> header, String key, double dflt) throws PlanException {
        String v = header.get(key);
        if (v == null) return dflt;
        double t;
        try {
            t = Double.parseDouble(v);
        } catch (NumberFormatException e) {
            throw new PlanException(key + "=" + v + " isn't a number");
        }
        if (!(t >= 0.0)) throw new PlanException(key + "=" + v + " must be >= 0");
        return t;
    }

    private static int integer(String s, String at) throws PlanException {
        try {
            return Integer.parseInt(s);
        } catch (NumberFormatException e) {
            throw new PlanException(at + ": " + s + " isn't a tick number");
        }
    }

    private static float fbits(String hex, String at) throws PlanException {
        try {
            return Float.intBitsToFloat(Integer.parseUnsignedInt(hex, 16));
        } catch (NumberFormatException e) {
            throw new PlanException(at + ": " + hex + " isn't float bits in hex");
        }
    }

    private static double dbits(String hex, String at) throws PlanException {
        try {
            return Double.longBitsToDouble(Long.parseUnsignedLong(hex, 16));
        } catch (NumberFormatException e) {
            throw new PlanException(at + ": " + hex + " isn't double bits in hex");
        }
    }

    private static int keys(String s, String at) throws PlanException {
        if (s.equals("-")) return 0;
        int bits = 0;
        for (char c : s.toCharArray()) {
            bits |= switch (c) {
                case 'W' -> W;
                case 'A' -> A;
                case 'S' -> S;
                case 'D' -> D;
                case 'J' -> J;
                case 'R' -> SPRINT;
                default -> throw new PlanException(at + ": key " + c + " isn't one of WASDJR");
            };
        }
        return bits;
    }

    /** Keys as a tape writes them: a subset of WASDJR, or "-". */
    public static String keyString(int bits) {
        StringBuilder sb = new StringBuilder();
        if ((bits & W) != 0) sb.append('W');
        if ((bits & A) != 0) sb.append('A');
        if ((bits & S) != 0) sb.append('S');
        if ((bits & D) != 0) sb.append('D');
        if ((bits & J) != 0) sb.append('J');
        if ((bits & SPRINT) != 0) sb.append('R');
        return sb.isEmpty() ? "-" : sb.toString();
    }
}
