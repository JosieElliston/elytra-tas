package jealoustone.elytratas.phase;

import jealoustone.elytratas.mixin.DeltaTrackerTimerAccessor;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.components.toasts.SystemToast;
import net.minecraft.network.chat.Component;

/**
 * Moves the client's tick grid without a restart, for the phase buttons in the config screen.
 *
 * Only while not in a world. why? a server could see the client's tick phase jump within one
 * session; between sessions it looks like any other launch. Only ever delays the grid: a
 * shift of -1 ms is a delay of 49. why? the timer then never runs two ticks less than 50 ms
 * apart; the one shift looks like a frame hitch.
 */
public final class ClientPhase {
    private static final SystemToast.SystemToastId TOAST = new SystemToast.SystemToastId();
    /** Total delay since launch, mod 50 ms. */
    private static int delayed;

    private ClientPhase() {
    }

    public static boolean canShift(Minecraft client) {
        return client.level == null && client.getConnection() == null;
    }

    /** Phase of the tick grid on System.nanoTime, in ms [0, 50); NaN before the first tick. */
    public static double phase(Minecraft client) {
        if (!(client.getDeltaTracker() instanceof DeltaTrackerTimerAccessor t)) return Double.NaN;
        double grid = t.elytraTas$lastMs() - t.elytraTas$deltaTickResidual() * 50.0;
        return Math.floorMod(Math.round(grid * 1000), 50_000) / 1000.0;
    }

    public static int delayed() {
        return delayed;
    }

    /**
     * Shifts the grid by ms (mod 50), as a delay. The timer adds elapsed time over 50 ms to its
     * residual and ticks when that reaches 1, truncating toward zero, so lowering the residual by
     * d/50 (below 0 is fine) puts every later tick d ms later. Client thread.
     */
    public static void shift(Minecraft client, int ms) {
        int d = Math.floorMod(ms, 50);
        if (d == 0 || !canShift(client) || !(client.getDeltaTracker() instanceof DeltaTrackerTimerAccessor t)) return;
        t.elytraTas$setDeltaTickResidual(t.elytraTas$deltaTickResidual() - d / 50f);
        delayed = (delayed + d) % 50;
        SystemToast.addOrUpdate(client.gui.toastManager(), TOAST, Component.literal("Client tick phase"),
                Component.literal(String.format("Delayed %d ms (%+d ms). Total since launch: %d ms.", d, ms, delayed)));
    }
}
