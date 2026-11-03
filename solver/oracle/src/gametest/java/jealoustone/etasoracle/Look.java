package jealoustone.etasoracle;

import java.util.SplittableRandom;

/**
 * Random rotations in segments of a few to 40 ticks: held, drifting, spinning fast,
 * random every tick, snapped to an extreme pitch, jittering, or easing toward a target
 * pitch. Yaw is never wrapped. why? the client's yaw is unbounded and elytra physics
 * reads its float bits through the sine table.
 */
final class Look {
    private final SplittableRandom r;
    float yaw;
    float pitch;
    private float lo = -90.0F, hi = 90.0F;
    private int mode, left;
    private float dYaw, dPitch, target;
    /** A raw pitch to send once, outside [-90, 90], for setXRot's clamp. */
    private Float raw;

    Look(SplittableRandom r, float yaw, float pitch) {
        this.r = r;
        this.yaw = yaw;
        this.pitch = pitch;
    }

    /** Keep pitch in [lo, hi] from now on. */
    void range(float lo, float hi) {
        this.lo = lo;
        this.hi = hi;
    }

    /** Force a new segment on the next step. */
    void reset() {
        left = 0;
    }

    /** Advance one tick; returns the pitch to send (usually {@link #pitch}). */
    float step() {
        if (left-- <= 0) newSegment();
        switch (mode) {
            case 1 -> {
                yaw += dYaw;
                pitch += dPitch;
            }
            case 2 -> yaw += dYaw;
            case 3 -> {
                yaw += (float) (r.nextDouble() * 360.0 - 180.0);
                pitch = (float) (lo + r.nextDouble() * (hi - lo));
            }
            case 5 -> {
                yaw += (float) (r.nextGaussian() * 0.3);
                pitch += (float) (r.nextGaussian() * 0.3);
            }
            case 6 -> {
                yaw += dYaw;
                float d = target - pitch;
                pitch += Math.abs(d) < dPitch ? d : Math.signum(d) * dPitch;
            }
            default -> {
            }
        }
        pitch = Math.clamp(pitch, lo, hi);
        if (raw != null) {
            float p = raw;
            raw = null;
            return p;
        }
        return pitch;
    }

    private void newSegment() {
        mode = r.nextInt(7);
        left = 2 + r.nextInt(mode == 3 ? 8 : 38);
        switch (mode) {
            case 1 -> {
                dYaw = (float) (r.nextGaussian() * 4.0);
                dPitch = (float) (r.nextGaussian() * 2.0);
            }
            case 2 -> dYaw = (float) ((r.nextBoolean() ? 1 : -1) * (20.0 + r.nextDouble() * 160.0));
            case 4 -> {
                float[] extremes = {lo, hi, -90.0F, 90.0F, -89.99F, 89.99F, 0.0F};
                pitch = extremes[r.nextInt(extremes.length)];
                if (r.nextInt(4) == 0) raw = r.nextBoolean() ? 120.0F : -200.0F;
            }
            case 6 -> {
                dYaw = (float) (r.nextGaussian() * 1.5);
                dPitch = (float) (1.0 + r.nextDouble() * 9.0);
                target = (float) (lo + r.nextDouble() * (hi - lo));
            }
            default -> {
            }
        }
    }
}
