package jealoustone.elytratas.play;

import com.mojang.brigadier.CommandDispatcher;
import com.mojang.brigadier.arguments.StringArgumentType;
import com.mojang.brigadier.builder.LiteralArgumentBuilder;
import net.fabricmc.fabric.api.client.command.v2.ClientCommands;
import net.fabricmc.fabric.api.client.command.v2.FabricClientCommandSource;
import net.minecraft.ChatFormatting;
import net.minecraft.commands.SharedSuggestionProvider;
import net.minecraft.network.chat.Component;
import net.minecraft.network.chat.MutableComponent;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.stream.Stream;

/** {@code /etas play <plan> [runs=N] [settle=T] [maxfail=K] [hold_glide=1]}, {@code /etas stop}, {@code /etas status}, {@code /etas queue [on|off]}. */
public final class PlayCommand {
    private PlayCommand() {
    }

    public static void register(CommandDispatcher<FabricClientCommandSource> dispatcher) {
        LiteralArgumentBuilder<FabricClientCommandSource> play = ClientCommands.literal("play")
                .executes(c -> {
                    c.getSource().sendFeedback(Component.literal("usage: /etas play <plan> [runs=N] [settle=T] [maxfail=K] [hold_glide=1: singleplayer test only]; plans are "
                            + "<name>.tas in " + Playback.dir(c.getSource().getClient())));
                    return 1;
                })
                .then(ClientCommands.argument("plan", StringArgumentType.word())
                        .suggests((c, b) -> SharedSuggestionProvider.suggest(plans(Playback.dir(c.getSource().getClient())), b))
                        .executes(c -> play(c.getSource(), StringArgumentType.getString(c, "plan"), ""))
                        .then(ClientCommands.argument("options", StringArgumentType.greedyString())
                                .executes(c -> play(c.getSource(), StringArgumentType.getString(c, "plan"),
                                        StringArgumentType.getString(c, "options")))));
        dispatcher.register(ClientCommands.literal("etas")
                .then(play)
                .then(ClientCommands.literal("stop").executes(c -> {
                    if (!Playback.active()) c.getSource().sendFeedback(Component.literal("etas: nothing is playing"));
                    Playback.stop("/etas stop");
                    return 1;
                }))
                .then(ClientCommands.literal("status").executes(c -> status(c.getSource())))
                .then(ClientCommands.literal("queue")
                        .executes(c -> {
                            c.getSource().sendFeedback(Component.literal("etas queue is " + (PlanQueue.on() ? "on" : "off")
                                    + "; /etas queue on|off; plans go in " + PlanQueue.dir(c.getSource().getClient())));
                            return 1;
                        })
                        .then(ClientCommands.literal("on").executes(c -> {
                            String refused = PlanQueue.enable(c.getSource().getClient());
                            if (refused != null) {
                                c.getSource().sendError(Component.literal("etas queue: " + refused));
                                return 0;
                            }
                            return 1;
                        }))
                        .then(ClientCommands.literal("off").executes(c -> {
                            PlanQueue.disable("/etas queue off");
                            Playback.stop("/etas queue off");
                            return 1;
                        }))));
    }

    private static List<String> plans(Path dir) {
        try (Stream<Path> s = Files.list(dir)) {
            return s.map(p -> p.getFileName().toString()).filter(n -> n.endsWith(".tas"))
                    .map(n -> n.substring(0, n.length() - 4)).sorted().toList();
        } catch (IOException e) {
            return List.of();
        }
    }

    private static int play(FabricClientCommandSource src, String name, String options) {
        Playback.Options o = Playback.Options.DEFAULT;
        for (String opt : options.strip().split("\\s+")) {
            if (opt.isEmpty()) continue;
            int eq = opt.indexOf('=');
            int v;
            try {
                v = eq < 0 ? -1 : Integer.parseInt(opt.substring(eq + 1));
            } catch (NumberFormatException e) {
                v = -1;
            }
            String key = eq < 0 ? opt : opt.substring(0, eq);
            if (v < 0 || (v == 0 && !key.equals("runs"))) {
                src.sendError(Component.literal("etas play: bad option " + opt + " (runs=N, settle=T, maxfail=K, hold_glide=1)"));
                return 0;
            }
            switch (key) {
                case "runs" -> o = new Playback.Options(v, o.settle(), o.maxFail(), o.holdGlide());
                case "settle" -> o = new Playback.Options(o.runs(), v, o.maxFail(), o.holdGlide());
                case "maxfail" -> o = new Playback.Options(o.runs(), o.settle(), v, o.holdGlide());
                case "hold_glide" -> {
                    if (v != 1) {
                        src.sendError(Component.literal("etas play: bad option " + opt + " (hold_glide=1)"));
                        return 0;
                    }
                    o = new Playback.Options(o.runs(), o.settle(), o.maxFail(), true);
                }
                default -> {
                    src.sendError(Component.literal("etas play: unknown option " + opt + " (runs=N, settle=T, maxfail=K, hold_glide=1)"));
                    return 0;
                }
            }
        }
        String refused = Playback.start(src.getClient(), name, o);
        if (refused != null) {
            src.sendError(Component.literal("etas play: " + refused));
            return 0;
        }
        return 1;
    }

    private static int status(FabricClientCommandSource src) {
        Playback.Status s = Playback.status();
        MutableComponent out = Component.empty().append(Component.literal("etas play").withStyle(ChatFormatting.BOLD));
        if (s.plan() == null) {
            src.sendFeedback(out.append(" — nothing played yet this session"));
            return 1;
        }
        String state = switch (s.phase()) {
            case IDLE -> "stopped" + (s.stopReason().isEmpty() ? "" : ": " + s.stopReason());
            case RESET -> "resetting to the start";
            case SETTLE -> "at the start, settling";
            case PLAY -> String.format("line %d of %d (segment %s), %d divergent ticks (max %.2e), %d reactions", s.line(),
                    s.lines(), s.segment(), s.divergences(), s.maxDev(), s.reactions());
        };
        out.append(" " + s.plan() + ": " + state);
        out.append("\n  attempt " + s.attempt() + ", " + s.done() + (s.runs() > 0 ? " of " + s.runs() : "") + " runs done, "
                + s.fails() + " failed in a row");
        List<Playback.Attempt> h = s.history();
        for (Playback.Attempt a : h.subList(Math.max(0, h.size() - 5), h.size())) {
            out.append(Component.literal(String.format("\n  #%d %s at line %d: %s; %d divergent, max %.2e; %d reactions%s",
                    a.n(), a.ok() ? "ok" : "failed", a.lines(), a.reason(), a.divergences(), a.maxDev(),
                    a.reactions().size(), a.path().size() > 1 ? "; path " + String.join(" ", a.path()) : ""))
                    .withStyle(a.ok() ? ChatFormatting.GRAY : ChatFormatting.RED));
        }
        src.sendFeedback(out);
        return 1;
    }
}
