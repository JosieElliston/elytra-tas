package jealoustone.elytratas.play;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * A plan of solver/PLAN-FORMAT.md version 1, or a plain {@code .tas} tape (format in
 * extract_run.py's docstring). Arrays are indexed by line: index 0 is the start pose,
 * line {@code t} is tick {@code t} after it.
 */
public final class Plan {
    public static final String FORMAT = "etas-plan 1";
    public static final double DEFAULT_TOL = 1e-4;
    /** Header keys only a plan has; a plain tape with any of them is refused. */
    private static final Set<String> PLAN_KEYS = Set.of("account", "course", "course_sha256", "world_sha256", "diverge_tol");

    /** Input bits, as the logger's {@code T} records and the oracle's Driver use them. */
    public static final int W = 1, S = 2, A = 4, D = 8, J = 16, SPRINT = 64;

    public record React(int t0, int t1, String event, String action, int fileLine) {
    }

    public static final class PlanException extends Exception {
        PlanException(String message) {
            super(message);
        }
    }

    public final String name;
    public final Map<String, String> header;
    /** True for {@code format=etas-plan 1}, false for a plain tape. */
    public final boolean versioned;
    public final int lines;
    public final float[] yaw, pitch;
    public final int[] keys;
    public final double[] x, y, z;
    public final double tol;
    public final List<React> reactions;

    private Plan(String name, Map<String, String> header, boolean versioned, float[] yaw, float[] pitch, int[] keys,
                 double[] x, double[] y, double[] z, double tol, List<React> reactions) {
        this.name = name;
        this.header = header;
        this.versioned = versioned;
        this.lines = yaw.length - 1;
        this.yaw = yaw;
        this.pitch = pitch;
        this.keys = keys;
        this.x = x;
        this.y = y;
        this.z = z;
        this.tol = tol;
        this.reactions = reactions;
    }

    /** Header value or null. */
    public String get(String key) {
        return header.get(key);
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
        List<String[]> ticks = new ArrayList<>();
        List<Integer> tickAt = new ArrayList<>();
        List<String[]> directives = new ArrayList<>();
        List<Integer> directiveAt = new ArrayList<>();
        for (int n = 0; n < raw.size(); n++) {
            String line = raw.get(n);
            int hash = line.indexOf('#');
            if (hash >= 0) line = line.substring(0, hash);
            line = line.strip();
            if (line.isEmpty()) continue;
            if (line.startsWith("@")) {
                directives.add(line.split("\\s+"));
                directiveAt.add(n + 1);
            } else if (line.contains("=")) {
                int eq = line.indexOf('=');
                header.put(line.substring(0, eq).strip(), line.substring(eq + 1).strip());
            } else {
                ticks.add(line.split("\\s+"));
                tickAt.add(n + 1);
            }
        }

        String format = header.get("format");
        boolean versioned = format != null;
        if (versioned && !format.equals(FORMAT)) {
            throw new PlanException("format=" + format + ": this mod reads format=" + FORMAT);
        }
        if (!versioned) {
            for (String k : header.keySet()) {
                if (PLAN_KEYS.contains(k)) throw new PlanException(k + "= is a plan key, but there is no format=" + FORMAT);
            }
            if (!directives.isEmpty()) {
                throw new PlanException("line " + directiveAt.getFirst() + ": an @ line, but there is no format=" + FORMAT);
            }
        }

        int count = ticks.size();
        if (count == 0) throw new PlanException("no tick lines");
        float[] yaw = new float[count + 1], pitch = new float[count + 1];
        int[] keys = new int[count + 1];
        double[] x = new double[count + 1], y = new double[count + 1], z = new double[count + 1];

        String start = header.get("start");
        if (start == null) throw new PlanException("no start= (the start contract)");
        String[] s = start.split("\\s+");
        if (s.length < 5) throw new PlanException("start=" + start + ": want x y z yaw pitch as bits");
        x[0] = dbits(s[0], "start");
        y[0] = dbits(s[1], "start");
        z[0] = dbits(s[2], "start");
        yaw[0] = fbits(s[3], "start");
        pitch[0] = fbits(s[4], "start");

        for (int i = 1; i <= count; i++) {
            String[] f = ticks.get(i - 1);
            String at = "line " + tickAt.get(i - 1);
            if (f.length < 7) throw new PlanException(at + ": want tick yaw pitch keys x y z");
            int t;
            try {
                t = Integer.parseInt(f[0]);
            } catch (NumberFormatException e) {
                throw new PlanException(at + ": tick " + f[0] + " isn't a number");
            }
            if (t != i) throw new PlanException(at + ": tick " + t + ", expected " + i);
            yaw[i] = fbits(f[1], at);
            pitch[i] = fbits(f[2], at);
            keys[i] = keys(f[3], at);
            x[i] = dbits(f[4], at);
            y[i] = dbits(f[5], at);
            z[i] = dbits(f[6], at);
        }

        String ticksKey = header.get("ticks");
        if (ticksKey != null && !ticksKey.equals(Integer.toString(count))) {
            throw new PlanException("header says ticks=" + ticksKey + ", the file has " + count + " tick lines");
        }

        double tol = DEFAULT_TOL;
        String tolKey = header.get("diverge_tol");
        if (tolKey != null) {
            try {
                tol = Double.parseDouble(tolKey);
            } catch (NumberFormatException e) {
                throw new PlanException("diverge_tol=" + tolKey + " isn't a number");
            }
            if (!(tol >= 0.0)) throw new PlanException("diverge_tol=" + tolKey + " must be >= 0");
        }

        List<React> reactions = new ArrayList<>();
        for (int k = 0; k < directives.size(); k++) {
            String[] d = directives.get(k);
            String at = "line " + directiveAt.get(k);
            if (!d[0].equals("@react")) {
                throw new PlanException(at + ": " + d[0] + " isn't in format " + FORMAT + " (known: @react)");
            }
            if (d.length != 5) throw new PlanException(at + ": want @react t0 t1 event action");
            int t0, t1;
            try {
                t0 = Integer.parseInt(d[1]);
                t1 = Integer.parseInt(d[2]);
            } catch (NumberFormatException e) {
                throw new PlanException(at + ": t0 and t1 must be tick numbers");
            }
            if (t0 < 1 || t1 < t0 || t1 > count) {
                throw new PlanException(at + ": window " + t0 + ".." + t1 + " isn't within ticks 1.." + count);
            }
            if (!d[3].equals("glide_off")) throw new PlanException(at + ": unknown event " + d[3] + " (known: glide_off)");
            if (!d[4].equals("press_jump")) throw new PlanException(at + ": unknown action " + d[4] + " (known: press_jump)");
            // press_jump has to be an edge on any tick of the window
            for (int t = t0; t <= t1; t++) {
                if ((keys[t - 1] & J) != 0) {
                    throw new PlanException(at + ": press_jump at tick " + t + ", but tick " + (t - 1) + " holds J");
                }
            }
            reactions.add(new React(t0, t1, d[3], d[4], directiveAt.get(k)));
        }
        return new Plan(name, header, versioned, yaw, pitch, keys, x, y, z, tol, List.copyOf(reactions));
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
