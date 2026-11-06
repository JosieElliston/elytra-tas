package jealoustone.elytratas.play;

import com.google.gson.GsonBuilder;
import com.google.gson.JsonObject;
import jealoustone.elytratas.netlog.NetLog;
import net.minecraft.client.Minecraft;
import net.minecraft.network.chat.Component;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.List;
import java.util.stream.Stream;

/**
 * {@code /etas queue on}: flies the plans a program drops in {@code <profile>/elytra-tas/queue/},
 * one attempt each, in name order, and answers each with a result file. See docs/playback.md,
 * "The queue". Every plan goes through {@link Playback#start}'s checks. Client thread only.
 */
public final class PlanQueue {
    /** Ticks between looks at an empty folder. */
    private static final int POLL = 10;
    private static final Playback.Options ONCE = new Playback.Options(1, 10, 1);

    private static boolean on;
    private static Path flying;
    private static int wait;

    private PlanQueue() {
    }

    public static Path dir(Minecraft mc) {
        return mc.gameDirectory.toPath().resolve("elytra-tas").resolve("queue");
    }

    public static boolean on() {
        return on;
    }

    /** Null on success, else why not. */
    public static String enable(Minecraft mc) {
        if (on) return "already on";
        if (Playback.active()) return "a plan is playing (/etas stop)";
        try {
            Files.createDirectories(dir(mc).resolve("done"));
        } catch (IOException e) {
            return "can't make " + dir(mc) + ": " + e;
        }
        on = true;
        flying = null;
        wait = 0;
        writeState(mc, "on");
        NetLog.playback("queue", "state=on");
        say("on; flying plans dropped in " + dir(mc) + ". /etas stop or /etas queue off ends it.");
        return null;
    }

    public static void disable(String reason) {
        if (!on) return;
        on = false;
        flying = null;
        Minecraft mc = Minecraft.getInstance();
        writeState(mc, "off " + reason);
        NetLog.playback("queue", "state=off", "reason=" + reason);
        say("off: " + reason);
    }

    /** START_CLIENT_TICK. */
    public static void tick(Minecraft mc) {
        if (!on || Playback.active()) return;
        try {
            if (flying != null) finishPlan(mc);
            if (on && ++wait >= POLL) {
                wait = 0;
                next(mc);
            }
        } catch (IOException | RuntimeException e) {
            NetLog.LOGGER.error("elytra-tas: queue failed", e);
            disable("error: " + e);
        }
    }

    private static void next(Minecraft mc) throws IOException {
        Path file;
        try (Stream<Path> s = Files.list(dir(mc))) {
            // why only .tas? a writer writes elsewhere and renames, so a plan is never seen half written
            file = s.filter(p -> p.getFileName().toString().endsWith(".tas") && Files.isRegularFile(p))
                    .sorted().findFirst().orElse(null);
        }
        if (file == null) return;
        String refused = Playback.startFile(mc, file, ONCE);
        if (refused != null) {
            JsonObject r = new JsonObject();
            r.addProperty("refused", refused);
            answer(mc, file, r);
            disable("refused " + file.getFileName() + ": " + refused);
            return;
        }
        flying = file;
    }

    private static void finishPlan(Minecraft mc) throws IOException {
        Path file = flying;
        flying = null;
        Playback.Status s = Playback.status();
        JsonObject r = new JsonObject();
        r.addProperty("stop_reason", s.stopReason());
        List<Playback.Attempt> h = s.history();
        if (!h.isEmpty()) {
            Playback.Attempt a = h.getLast();
            r.addProperty("ok", a.ok());
            r.addProperty("reason", a.reason());
            r.addProperty("abort_kind", a.abortKind());
            r.addProperty("lines", a.lines());
            r.addProperty("divergences", a.divergences());
            r.addProperty("max_dev", a.maxDev());
            r.addProperty("timer_line", a.timerLine());
            r.addProperty("finish_line", a.finishLine());
        }
        answer(mc, file, r);
        // why? anything else stopped the attempt from outside: /etas stop, a hub kick, a death, a dead reset item
        if (h.isEmpty() || !Playback.ATTEMPT_LIMIT.matcher(s.stopReason()).matches()) disable(s.stopReason());
    }

    /** The plan moves to done/ and its result lands beside it, last, so a result means the plan is finished. */
    private static void answer(Minecraft mc, Path plan, JsonObject r) throws IOException {
        Path done = dir(mc).resolve("done");
        String stem = plan.getFileName().toString().replaceFirst("\\.tas$", "");
        r.addProperty("plan", stem);
        Path log = NetLog.path();
        r.addProperty("log", log == null ? "" : log.getFileName().toString());
        Files.move(plan, done.resolve(stem + ".tas"), StandardCopyOption.REPLACE_EXISTING);
        Path tmp = done.resolve(stem + ".json.tmp");
        Files.writeString(tmp, new GsonBuilder().setPrettyPrinting().create().toJson(r) + "\n", StandardCharsets.UTF_8);
        Files.move(tmp, done.resolve(stem + ".json"), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING);
    }

    private static void writeState(Minecraft mc, String state) {
        try {
            Files.createDirectories(dir(mc));
            Files.writeString(dir(mc).resolve("state"), state + "\n", StandardCharsets.UTF_8);
        } catch (IOException e) {
            NetLog.LOGGER.error("elytra-tas: can't write the queue state", e);
        }
    }

    private static void say(String text) {
        Minecraft mc = Minecraft.getInstance();
        if (mc.player != null) mc.player.sendSystemMessage(Component.literal("etas queue: " + text));
        NetLog.LOGGER.info("etas queue: {}", text);
    }
}
