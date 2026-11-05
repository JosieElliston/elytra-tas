package jealoustone.etasoracle;

import jealoustone.elytratas.netlog.NetLog;
import jealoustone.etasoracle.mixin.ClientInputAccessor;
import jealoustone.etasoracle.mixin.EntityAccessor;
import jealoustone.etasoracle.mixin.LivingEntityAccessor;
import jealoustone.etasoracle.mixin.LocalPlayerAccessor;
import net.fabricmc.api.ClientModInitializer;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientTickEvents;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.ClientInput;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.core.BlockPos;
import net.minecraft.resources.Identifier;
import net.minecraft.world.entity.ai.attributes.AttributeInstance;
import net.minecraft.world.entity.ai.attributes.Attributes;
import net.minecraft.world.entity.player.Input;
import net.minecraft.world.phys.Vec2;

import java.util.ArrayList;
import java.util.List;

/**
 * Drives the local player the way {@code tas_play.pyj} does, and records what the
 * logger's {@code T} records lack.
 *
 * <ul>
 * <li>Input is set in {@code START_LEVEL_TICK}, at the head of
 * {@code ClientLevel.tickEntities}, right before the player's physics: rotation with
 * {@code setYRot}/{@code setXRot}, keys in a plain {@link ClientInput} swapped in for
 * {@code player.input}. A jump press (jump held now, not in the previous step) is sent
 * as {@code autoJumpTime = 1} with jump left off, because a plain ClientInput's
 * {@code tick()} is empty and {@code aiStep} would otherwise read the new key as
 * {@code wasJumping} and never see an edge (minescript/README.md, "Things that will bite").</li>
 * <li>{@code P} and {@code Q} records are taken at the head and return of
 * {@code LocalPlayer.tick}, keyed by the logger's tick number.</li>
 * </ul>
 * All of it runs on the client thread; the test thread only starts and stops a case
 * between ticks.
 */
public final class Driver implements ClientModInitializer {
    /** One step's input: rotation and the keys held, as the logger's input bits. */
    public record Step(float yaw, float pitch, int keys) {
    }

    public interface Policy {
        /** The input for step {@code i} (1-based), seeing the state the previous step left. */
        Step next(LocalPlayer player, int i);
    }

    public static final int W = 1, S = 2, A = 4, D = 8, J = 16, SHIFT = 32, SPRINT = 64;

    private static LocalPlayer player;
    private static ClientInput input;
    private static Policy policy;
    private static int step;
    private static int prevKeys;
    private static Step last;
    private static boolean press;
    private static final List<String> RECORDS = new ArrayList<>();
    private static final List<double[]> POSITIONS = new ArrayList<>();
    private static String error;

    private static final Identifier SPRINTING = Identifier.withDefaultNamespace("sprinting");

    @Override
    public void onInitializeClient() {
        ClientTickEvents.START_LEVEL_TICK.register(level -> {
            try {
                apply();
            } catch (RuntimeException e) {
                // an exception here would crash the game loop; the test thread reports it
                error = e.toString();
                policy = null;
            }
        });
    }

    /** Swap in the plain input for the rest of the session. Client thread. */
    public static void install(LocalPlayer p) {
        player = p;
        input = new ClientInput();
        p.input = input;
        setKeys(0);
    }

    /** Start driving with {@code pol} from the next tick. Client thread, between ticks. */
    public static void begin(Policy pol) {
        RECORDS.clear();
        POSITIONS.clear();
        error = null;
        step = 0;
        prevKeys = 0;
        last = null;
        policy = pol;
    }

    /** Stop driving and release every key. Client thread, between ticks. */
    public static void end() {
        policy = null;
        prevKeys = 0;
        setKeys(0);
    }

    public static int steps() {
        return step;
    }

    public static String error() {
        return error;
    }

    public static List<String> records() {
        return RECORDS;
    }

    public static List<double[]> positions() {
        return POSITIONS;
    }

    private static void apply() {
        if (policy == null) return;
        LocalPlayer p = Minecraft.getInstance().player;
        if (p != player) throw new IllegalStateException("player changed (respawn or dimension change)");
        if (p.input != input) throw new IllegalStateException("player.input was replaced");
        step++;
        Step s = policy.next(p, step);
        p.setYRot(s.yaw);
        p.setXRot(s.pitch);
        press = (s.keys & J) != 0 && (prevKeys & J) == 0;
        setKeys(press ? s.keys & ~J : s.keys);
        prevKeys = s.keys;
        last = s;
    }

    private static void setKeys(int keys) {
        ((LocalPlayerAccessor) player).oracle$setAutoJumpTime(press && policy != null ? 1 : 0);
        input.keyPresses = new Input((keys & W) != 0, (keys & S) != 0, (keys & A) != 0, (keys & D) != 0,
                (keys & J) != 0, (keys & SHIFT) != 0, (keys & SPRINT) != 0);
        // KeyboardInput.tick: Vec2(left impulse, forward impulse).normalized()
        float forward = impulse((keys & W) != 0, (keys & S) != 0);
        float left = impulse((keys & A) != 0, (keys & D) != 0);
        ((ClientInputAccessor) input).oracle$setMoveVector(new Vec2(left, forward).normalized());
    }

    private static float impulse(boolean positive, boolean negative) {
        return positive == negative ? 0.0F : positive ? 1.0F : -1.0F;
    }

    /** LocalPlayer.tick head ({@code head}) or return. */
    public static void physics(LocalPlayer p, boolean head) {
        if (policy == null || p != player || last == null) return;
        StringBuilder sb = new StringBuilder(200);
        sb.append(head ? 'P' : 'Q').append('\t').append(NetLog.tick);
        if (head) {
            sb.append('\t').append((double) last.yaw).append('\t').append((double) last.pitch)
                    .append('\t').append(last.keys)
                    // the lift's Math.cos, as this JVM computes it: it can differ from fdlibm
                    // in the last place (README.md, "What it doesn't cover")
                    .append('\t').append(Math.cos(p.getXRot() * (float) (Math.PI / 180.0)));
        }
        state(sb, p);
        RECORDS.add(sb.toString());
        if (!head) POSITIONS.add(new double[]{p.getX(), p.getY(), p.getZ()});
    }

    /** The fields port reads that the logger's T record doesn't carry (README.md, "Case files"). */
    private static void state(StringBuilder sb, LocalPlayer p) {
        AttributeInstance speed = p.getAttribute(Attributes.MOVEMENT_SPEED);
        BlockPos support = p.mainSupportingBlockPos.orElse(null);
        Input k = p.input.keyPresses;
        Vec2 move = p.input.getMoveVector();
        sb.append('\t').append(p.getPose().name())
                .append('\t').append(b(p.onGround())).append('\t').append(b(p.horizontalCollision))
                .append('\t').append(b(p.verticalCollision)).append('\t').append(b(p.verticalCollisionBelow))
                .append('\t').append(b(p.minorHorizontalCollision))
                .append('\t').append(support == null ? "-" : support.getX() + "," + support.getY() + "," + support.getZ())
                .append('\t').append(b(((EntityAccessor) p).oracle$onGroundNoBlocks()))
                .append('\t').append(p.fallDistance)
                .append('\t').append(b(p.isFallFlying())).append('\t').append(b(p.isSprinting()))
                .append('\t').append(b(speed != null && speed.getModifier(SPRINTING) != null))
                .append('\t').append(((LivingEntityAccessor) p).oracle$noJumpDelay())
                .append('\t').append(b(((LivingEntityAccessor) p).oracle$jumping()))
                .append('\t').append((double) p.xxa).append('\t').append((double) p.yya).append('\t').append((double) p.zza)
                .append('\t').append(bits(k))
                .append('\t').append((double) move.x).append('\t').append((double) move.y)
                .append('\t').append(((LocalPlayerAccessor) p).oracle$sprintTriggerTime())
                .append('\t').append(((LocalPlayerAccessor) p).oracle$autoJumpTime())
                .append('\t').append(b(((LocalPlayerAccessor) p).oracle$crouching()));
    }

    private static char b(boolean v) {
        return v ? '1' : '0';
    }

    /** The logger's input bits (NetLog.inputBits). */
    static int bits(Input i) {
        return (i.forward() ? W : 0) | (i.backward() ? S : 0) | (i.left() ? A : 0) | (i.right() ? D : 0)
                | (i.jump() ? J : 0) | (i.shift() ? SHIFT : 0) | (i.sprint() ? SPRINT : 0);
    }
}
