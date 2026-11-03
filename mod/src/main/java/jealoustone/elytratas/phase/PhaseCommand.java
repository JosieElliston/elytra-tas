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
        src.sendFeedback(Component.literal("/etas phase: tick timing between you and the server (/etas phase help)\n"
                + "/etas play <plan> [runs=N]: fly a plan, on " + jealoustone.elytratas.play.Playback.ALT + " only; /etas stop; /etas status"));
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

        boolean sending = PhaseTracker.sending();
        if (!sending) {
            out.append(Component.literal("\n" + PhaseTracker.user() + "@" + PhaseTracker.server() + " isn't whitelisted, "
                    + "so no pings or probes go out (Mod Menu → Elytra TAS → Whitelist)").withStyle(ChatFormatting.GRAY));
        }

        PhaseTracker.Snapshot l = last;
        String noFit = !sending ? "(probes off)" : s.probeReplies() == 0
                ? String.format("(no reply to %d tab completions)", s.probesSent())
                : s.probes() < 20 ? String.format("(%d probe replies)", s.probes()) : "(no tick-start jump)";
        // The loop in the order it happens. Net up and net down can only be measured together.
        List<Component[]> rows = new ArrayList<>();
        rows.add(row(label("net up + down", "The round trip: our own pings, 4 a second, timed at the socket. The "
                        + "client can't split it into up and down. ± is half the 10–90% range."),
                ms(s.rtt()), Double.isNaN(s.rtt()) ? sending ? "" : "(pings off)" : String.format("ms ±%.0f", s.rttSpread() / 1e6),
                delta(s.rtt(), l == null ? Double.NaN : l.rtt(), false)));
        rows.add(row(label("server wait", "From your move reaching the server to the start of the tick that reads "
                        + "it, 0–50 ms. Near 0 or 50, jitter decides which tick sees your landing."),
                ms(s.serverWait()), Double.isNaN(s.serverWait()) ? noFit : "ms",
                delta(s.serverWait(), l == null ? Double.NaN : l.serverWait(), true)));
        rows.add(row(label("server work", "How long each server tick takes to run (MSPT). Every reply waits for "
                        + "it: a tick's packets leave when it ends."),
                ms(s.serverWork()), Double.isNaN(s.serverWork()) ? noFit : "ms",
                delta(s.serverWork(), l == null ? Double.NaN : l.serverWork(), false)));
        rows.add(row(label("client wait", "From the server's reply reaching your client to the start of the "
                        + "client tick that first sees it, 0–50 ms."),
                ms(s.clientWait()), Double.isNaN(s.clientWait()) ? "" : s.clientWaitDerived() ? "ms (derived)" : "ms",
                delta(s.clientWait(), l == null ? Double.NaN : l.clientWait(), true)));
        rows.add(row(label("client work", "From the start of your client tick to its move leaving the socket."),
                ms(s.clientWork()), Double.isNaN(s.clientWork()) ? "" : "ms",
                delta(s.clientWork(), l == null ? Double.NaN : l.clientWork(), false)));
        rows.add(new Component[]{Component.literal("from set_health").withStyle(ChatFormatting.BOLD)});
        rows.add(row(label("wait + work", "Server wait + server work from set_health, which leaves at the end of "
                        + "the tick that read your move: its time minus your move's minus the round trip."),
                ms(s.waitPlusWork()), Double.isNaN(s.waitPlusWork()) ? "(no per-tick packet)" : "ms",
                null));
        rows.add(row(label("tick interval", "Time between server ticks, from the spacing of set_health. 50 ms "
                        + "unless the server is lagging. Longest includes network jitter."),
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
     * row doesn't widen its column, so a long note doesn't push the change column out. A
     * one-cell row is a section heading, unindented.
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
            if (row.length == 1) {
                out.append("\n").append(row[0]);
                continue;
            }
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
        out.append("\nThe rows follow one loop in the order it happens: your move goes up, waits for a server "
                + "tick, the tick runs, its reply comes down, waits for a client tick, and that tick runs.");
        section(out, "net up + down", "The round trip, from our own pings (4 a second, timed at the socket; the "
                + "server answers them on its network thread, never waiting for a tick). ± is half the 10–90% "
                + "range. The client can only measure the two together, and nothing else here needs them apart.");
        section(out, "server wait", "From your move reaching the server to the start of the tick that reads it. "
                + "Near 0 or 50, jitter decides which tick sees your landing, and a bounce can come out perfect.");
        section(out, "server work", "How long each server tick runs (MSPT). A tick's replies leave when it ends, "
                + "so every reply waits for it.");
        section(out, "client wait", "From the server's reply reaching your client to the start of the client "
                + "tick that first sees it. Near 0 or 50, jitter decides which of your ticks first sees a "
                + "glide-off, so whether a redeploy press works. Measured from set_health; without it, derived "
                + "from the other rows (they add up to a whole number of ticks) and marked so.");
        section(out, "client work", "From the start of your client tick to its move leaving the socket.");
        section(out, "probes", "Server wait and work come from tab-completion requests, 4 a second at random "
                + "times, for /msg (or /tell, /help, /: the first the server answers). The server answers them "
                + "between ticks, so one arriving mid-tick waits for the tick to end. Fitting that delay against "
                + "when each was sent gives the tick start and the work time. The fit uses at least the last 20 s, "
                + "and finds work times of 8 ms or more. A server that answers completions off its main thread, "
                + "or not at all, gives no fit, and the rows say why.");
        section(out, "whitelist", "Pings and probes are packets the vanilla client wouldn't send, so they go out "
                + "only for whitelisted user@server pairs (Mod Menu → Elytra TAS → Whitelist; deftenvy@elytramc.net "
                + "by default, and singleplayer always). Elsewhere the round trip, server wait and work stay empty.");
        section(out, "moving your phase", "Mod Menu → Elytra TAS → Tick phase, while not in a world: buttons delay "
                + "your client's ticks by 1, 10, 49 (-1) or 40 (-10) ms, without a restart.");
        section(out, "from set_health", "Wait + work is the sum the probes split: set_health leaves at the end of "
                + "the tick that read your move. Tick interval is the spacing of set_health: 50 ms unless the server "
                + "lags. Both need set_health every tick: vanilla sends it only when health or food changed; EMC "
                + "sends it every tick.");
        section(out, "change", "The right-hand column, once you've run /etas phase before: the change since then, "
                + "wrapped to ±25 ms for the waits.");
        section(out, "window", "The last N seconds (/etas phase window N, default 10, at most "
                + MAX_WINDOW_S + "), cut short at a reconfiguration (how a proxy switches backends) or a phase jump: the server's tick phase "
                + "against your client's tick grid moving 8+ ms between consecutive 5 s blocks. A jump shows up "
                + "about 5 s after it happens. The header says when the window was cut.");
        src.sendFeedback(out);
        return 1;
    }

    private static void section(MutableComponent out, String title, String body) {
        out.append(Component.literal("\n" + title).withStyle(ChatFormatting.GRAY)).append("  " + body);
    }
}
