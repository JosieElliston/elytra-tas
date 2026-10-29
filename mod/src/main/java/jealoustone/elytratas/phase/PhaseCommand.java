package jealoustone.elytratas.phase;

import com.mojang.brigadier.CommandDispatcher;
import com.mojang.brigadier.arguments.IntegerArgumentType;
import com.mojang.brigadier.builder.LiteralArgumentBuilder;
import net.fabricmc.fabric.api.client.command.v2.ClientCommands;
import net.fabricmc.fabric.api.client.command.v2.FabricClientCommandSource;
import net.minecraft.ChatFormatting;
import net.minecraft.client.gui.Font;
import net.minecraft.network.chat.Component;
import net.minecraft.network.chat.HoverEvent;
import net.minecraft.network.chat.MutableComponent;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/** {@code /etas phase}, {@code /etas phase window [s]}, {@code /etas phase help}, {@code /etas help [phase]}. */
public final class PhaseCommand {
    private static final int MAX_WINDOW_S = (int) (PhaseTracker.KEEP / 1_000_000_000L);
    private static int windowSeconds = 10;
    private static PhaseTracker.Snapshot last;

    private PhaseCommand() {
    }

    static void forgetLast() {
        last = null;
    }

    public static void register(CommandDispatcher<FabricClientCommandSource> dispatcher) {
        LiteralArgumentBuilder<FabricClientCommandSource> phase = ClientCommands.literal("phase")
                .executes(c -> show(c.getSource()))
                .then(ClientCommands.literal("help").executes(c -> help(c.getSource())))
                .then(ClientCommands.literal("window")
                        .executes(c -> {
                            c.getSource().sendFeedback(Component.literal("etas phase window: " + windowSeconds + " s"));
                            return 1;
                        })
                        .then(ClientCommands.argument("seconds", IntegerArgumentType.integer(1, MAX_WINDOW_S))
                                .executes(c -> {
                                    windowSeconds = IntegerArgumentType.getInteger(c, "seconds");
                                    c.getSource().sendFeedback(Component.literal("etas phase window set to " + windowSeconds + " s"));
                                    return 1;
                                })));
        dispatcher.register(ClientCommands.literal("etas")
                .executes(c -> usage(c.getSource()))
                .then(phase)
                .then(ClientCommands.literal("help")
                        .executes(c -> usage(c.getSource()))
                        .then(ClientCommands.literal("phase").executes(c -> help(c.getSource())))));
    }

    private static int usage(FabricClientCommandSource src) {
        src.sendFeedback(Component.literal("/etas phase: tick timing between you and the server (/etas phase help)"));
        return 1;
    }

    private static int show(FabricClientCommandSource src) {
        PhaseTracker.Snapshot s = PhaseTracker.snapshot(windowSeconds * 1_000_000_000L);
        double age = (s.at() - s.from()) / 1e9;
        String header = s.cutReason().isEmpty()
                ? "  last " + windowSeconds + " s"
                : String.format("  last %.0f s (%s)", Math.max(age, 0), s.cutReason());
        if (last != null) header += ", change since " + duration((s.at() - last.at()) / 1e9) + " ago";
        MutableComponent out = Component.empty()
                .append(Component.literal("etas phase").withStyle(ChatFormatting.BOLD)).append(header);

        PhaseTracker.Snapshot l = last;
        List<Component[]> rows = new ArrayList<>();
        rows.add(row(label("round trip", "Network time there and back: our own pings, 4 a second, timed at the "
                        + "socket. It can't be split into up and down. ± is half the 10–90% range."),
                ms(s.rtt()), Double.isNaN(s.rtt()) ? "" : String.format("ms ±%.0f", s.rttSpread() / 1e6),
                delta(s.rtt(), l == null ? Double.NaN : l.rtt(), false)));
        String noFit = s.probes() < 12 ? String.format("(%d probe replies)", s.probes()) : "(no tick-start jump)";
        String waitNote = Double.isNaN(s.serverWait()) && !Double.isNaN(s.waitPlusWork())
                ? noFit + String.format(", wait + work %.0f ms", s.waitPlusWork() / 1e6) : noFit;
        rows.add(row(label("server wait", "From your move reaching the server to the start of the tick that reads "
                        + "it, 0–50 ms. Near 0 or 50, jitter decides which tick sees your landing."),
                ms(s.serverWait()), Double.isNaN(s.serverWait()) ? waitNote : "ms",
                delta(s.serverWait(), l == null ? Double.NaN : l.serverWait(), true)));
        rows.add(row(label("server work", "How long each server tick takes to run: what servers call MSPT."),
                ms(s.serverWork()), Double.isNaN(s.serverWork()) ? noFit
                        : String.format("ms (MSPT), from %d probes", s.probes()),
                delta(s.serverWork(), l == null ? Double.NaN : l.serverWork(), false)));
        rows.add(row(label("client wait", "From the server's reply reaching your client to the start of the "
                        + "client tick that first sees it, 0–50 ms."),
                ms(s.clientWait()), Double.isNaN(s.clientWait()) ? "" : s.clientWaitDerived() ? "ms (derived)" : "ms",
                delta(s.clientWait(), l == null ? Double.NaN : l.clientWait(), true)));
        rows.add(row(label("client work", "From the start of your client tick to its move leaving the socket."),
                ms(s.clientWork()), Double.isNaN(s.clientWork()) ? "" : "ms",
                delta(s.clientWork(), l == null ? Double.NaN : l.clientWork(), false)));
        double loop = s.rtt() + s.serverWait() + s.serverWork() + s.clientWait() + s.clientWork();
        if (!Double.isNaN(loop) && !s.clientWaitDerived()) {
            long ticks = Math.round(loop / PhaseTracker.TICK);
            rows.add(row(label("loop", "The five rows above add up to a whole number of ticks: client tick, "
                            + "move, server tick, reply, next client tick. The remainder is measurement error."),
                    ms(loop), String.format("ms = %d ticks %+.0f", ticks, (loop - ticks * PhaseTracker.TICK) / 1e6),
                    null));
        }
        rows.add(row(label("tick interval", "Time between server ticks, from the spacing of the per-tick packet. "
                        + "50 ms unless the server is lagging. Longest includes network jitter."),
                ms(s.tickInterval()), Double.isNaN(s.tickInterval()) ? "(no per-tick packet)"
                        : String.format("ms, longest %.0f ms", s.longest() / 1e6),
                null));
        table(out, src.getClient().font, rows);
        last = s;
        src.sendFeedback(out);
        return 1;
    }

    private static Component label(String text, String hover) {
        return Component.literal(text).withStyle(st -> st.withColor(ChatFormatting.GRAY)
                .withHoverEvent(new HoverEvent.ShowText(Component.literal(hover))));
    }

    private static Component[] row(Component label, String number, String rest, Component change) {
        Component[] cells = {label, Component.literal(number), Component.literal(rest), change};
        return change == null ? Arrays.copyOf(cells, 3) : cells;
    }

    private static String ms(double nanos) {
        return Double.isNaN(nanos) ? "—" : String.format("%.0f", nanos / 1e6);
    }

    private static Component delta(double now, double before, boolean circular) {
        if (Double.isNaN(now) || Double.isNaN(before)) return null;
        double x = circular ? PhaseTracker.wrap(now - before) : now - before;
        return Component.literal(String.format("%+.0f ms", x / 1e6)).withStyle(st -> st.withColor(ChatFormatting.GRAY)
                .withHoverEvent(new HoverEvent.ShowText(Component.literal("Change since your previous /etas phase."))));
    }

    /**
     * Lays rows out in columns by pixel width (chat uses a proportional font): label, number
     * (right-aligned), unit and notes, change (right-aligned). A left-aligned cell that ends its
     * row doesn't widen its column, so a long note doesn't push the change column out.
     */
    private static void table(MutableComponent out, Font font, List<Component[]> rows) {
        boolean[] right = {false, true, false, true};
        int sp = font.width(" ");
        int[] gapAfter = {4 * sp, sp, 4 * sp};
        int[] width = new int[right.length];
        for (Component[] row : rows) {
            for (int j = 0; j < row.length; j++) {
                if (right[j] || j < row.length - 1) width[j] = Math.max(width[j], font.width(row[j]));
            }
        }
        for (Component[] row : rows) {
            out.append("\n  ");
            int x = 0;
            int start = 0;
            for (int j = 0; j < row.length; j++) {
                int w = font.width(row[j]);
                int target = right[j] ? start + width[j] - w : start;
                pad(out, font, target - x);
                out.append(row[j]);
                x = Math.max(x, target) + w;
                if (j < gapAfter.length) start += width[j] + gapAfter[j];
            }
        }
    }

    /**
     * Appends px pixels of blank space, exactly when px is at least 3 bold spaces wide: a bold
     * space is a pixel wider than a plain one, so the two together make any width from there.
     */
    private static void pad(MutableComponent out, Font font, int px) {
        if (px <= 0) return;
        int sp = font.width(" ");
        int bold = font.width(Component.literal(" ").withStyle(ChatFormatting.BOLD));
        for (int b = 0; b * bold <= px; b++) {
            if ((px - b * bold) % sp != 0) continue;
            out.append(" ".repeat((px - b * bold) / sp));
            if (b > 0) out.append(Component.literal(" ".repeat(b)).withStyle(ChatFormatting.BOLD));
            return;
        }
        out.append(" ".repeat(Math.round((float) px / sp)));
    }

    private static String duration(double s) {
        if (s < 60) return String.format("%.0fs", s);
        if (s < 3600) return String.format("%dm %02ds", (long) s / 60, (long) s % 60);
        return String.format("%dh %02dm", (long) s / 3600, (long) s / 60 % 60);
    }

    private static int help(FabricClientCommandSource src) {
        MutableComponent out = Component.empty()
                .append(Component.literal("/etas phase").withStyle(ChatFormatting.BOLD))
                .append(" — where your packets land in each 50 ms server tick");
        section(out, "round trip", "Network time there and back, from our own pings (4 a second, timed at the "
                + "socket; the server answers them on its network thread, never waiting for a tick). ± is half the "
                + "10–90% range. It can't be split into up and down from the client, and nothing below needs it to "
                + "be.");
        section(out, "server wait", "From your move reaching the server to the start of the tick that reads it. "
                + "Near 0 or 50, jitter decides which tick sees your landing, and a bounce can come out perfect. "
                + "From the probes' tick start and the phase your moves are sent at.");
        section(out, "server work", "How long each server tick runs, what servers call MSPT. A tick's replies "
                + "leave when it ends, so this delays every reply.");
        section(out, "client wait", "From the server's reply reaching your client to the start of the client "
                + "tick that first sees it. Near 0 or 50, jitter decides which of your ticks first sees a "
                + "glide-off, so whether a redeploy press works. Measured from the per-tick packet; without one "
                + "it's derived from the loop and marked so.");
        section(out, "client work", "From the start of your client tick to its move leaving the socket.");
        section(out, "loop", "Round trip + the four rows above: a whole number of ticks, give or take the "
                + "measurement error shown. Restarting the game shifts your tick phase, which trades server wait "
                + "against client wait; the sum stays.");
        section(out, "tick interval", "Time between server ticks, from the spacing of the per-tick packet: 50 "
                + "ms unless the server lags. Longest includes network jitter.");
        section(out, "probes", "Tab-completion requests for /etas, 4 a second at random times. The server "
                + "answers them between ticks, so one arriving mid-tick waits for the tick to end. Fitting that "
                + "delay against when each was sent gives the tick start and the work time. A server that "
                + "answers completions off its main thread (some proxies and plugins) shows no jump, and the two "
                + "rows say so.");
        section(out, "change", "The right-hand column, once you've run /etas phase before: the change since then, "
                + "wrapped to ±25 ms for the waits.");
        section(out, "window", "The last N seconds (/etas phase window N, default 10, at most "
                + MAX_WINDOW_S + "), cut short at a reconfiguration (how a proxy switches backends) or a phase jump: the server's tick phase "
                + "against your client's tick grid moving 8+ ms between consecutive 5 s blocks. A jump shows up "
                + "about 5 s after it happens. The header says when the window was cut.");
        section(out, "per-tick packet", "Client wait and tick interval time the server's set_health packet, "
                + "which leaves at the end of each tick. Vanilla sends it only when health or food changed; EMC "
                + "sends it every tick.");
        src.sendFeedback(out);
        return 1;
    }

    private static void section(MutableComponent out, String title, String body) {
        out.append(Component.literal("\n" + title).withStyle(ChatFormatting.GRAY)).append("  " + body);
    }
}
