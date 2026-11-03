package jealoustone.etasoracle;

import net.minecraft.client.player.LocalPlayer;
import net.minecraft.core.BlockPos;
import net.minecraft.world.phys.Vec3;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.SplittableRandom;

import static jealoustone.etasoracle.Driver.A;
import static jealoustone.etasoracle.Driver.D;
import static jealoustone.etasoracle.Driver.J;
import static jealoustone.etasoracle.Driver.S;
import static jealoustone.etasoracle.Driver.SHIFT;
import static jealoustone.etasoracle.Driver.SPRINT;
import static jealoustone.etasoracle.Driver.W;

/**
 * One case: its blocks, its start, and the input policy, all from {@code (kind, seed)}.
 * The origin is a function of both, so a rerun lands on the same coordinates (the
 * physics' float bits depend on them) and no two cases of a run share a region.
 * Everything starts standing: on the case's floor, or on a one-block platform that is
 * removed right before the first tick.
 */
abstract class Scenario implements Driver.Policy {
    static final List<String> KINDS = List.of("air", "walls", "ground", "landing");

    final String kind;
    final long seed;
    final SplittableRandom r;
    final BlockPos origin;
    /** Planned placements, in placement order; what the world ends up with is read back. */
    final Map<BlockPos, String> blocks = new LinkedHashMap<>();
    /** Removed before the first tick; null when the start stands on the case's floor. */
    BlockPos platform;
    Vec3 start;
    float startYaw;
    int length;
    /** The case stops when the player drops below this. */
    double minY;

    final Look look;
    private int prevKeys;
    private int airborne;
    private boolean deployArmed;
    private int deployWait;

    Scenario(String kind, long seed, int y) {
        this.kind = kind;
        this.seed = seed;
        this.r = new SplittableRandom(seed * 0x9E3779B97F4A7C15L + KINDS.indexOf(kind));
        int k = KINDS.indexOf(kind);
        // 2048 apart: seeds 0-1023 per kind get their own cell
        origin = new BlockPos(2048 * (int) (seed % 32) - 32768 + 1024, y, 2048 * (k * 32 + (int) (seed / 32 % 32)) - 131072 + 1024);
        startYaw = (float) (r.nextDouble() * 360.0 - 180.0);
        start = new Vec3(origin.getX() + 0.2 + 0.6 * r.nextDouble(), origin.getY(), origin.getZ() + 0.2 + 0.6 * r.nextDouble());
        look = new Look(r, startYaw, (float) (r.nextDouble() * 40.0 - 20.0));
    }

    static Scenario create(String kind, long seed) {
        Scenario s = switch (kind) {
            case "air" -> new Air(seed);
            case "walls" -> new Walls(seed);
            case "ground" -> new Ground(seed);
            case "landing" -> new Landing(seed);
            default -> throw new IllegalArgumentException("unknown case kind " + kind + "; known: " + KINDS);
        };
        s.build();
        return s;
    }

    String name() {
        return kind + "-" + seed;
    }

    abstract void build();

    /** The keys for step {@code i}, before the deploy logic. */
    abstract int keys(LocalPlayer p, int i);

    /** Sets the pitch range for this step. */
    abstract void aim(LocalPlayer p, int i);

    boolean stop(LocalPlayer p) {
        return p.getY() < minY;
    }

    @Override
    public final Driver.Step next(LocalPlayer p, int i) {
        aim(p, i);
        float pitch = look.step();
        int keys = keys(p, i);
        keys = deploy(p, keys);
        prevKeys = keys;
        return new Driver.Step(look.yaw, pitch, keys);
    }

    /** How eagerly a fall is turned into a glide: chance per airborne spell, and delay. */
    double deployChance() {
        return 1.0;
    }

    /**
     * Airborne and not gliding: after a random wait, press jump (an edge: released first
     * if it was held) until the glide starts. why the edge? tryToStartFallFlying runs only
     * on a jump press in air.
     */
    private int deploy(LocalPlayer p, int keys) {
        if (p.onGround() || p.isFallFlying()) {
            airborne = 0;
            return keys;
        }
        if (airborne++ == 0) {
            deployArmed = r.nextDouble() < deployChance();
            deployWait = 1 + r.nextInt(6);
        }
        if (!deployArmed || airborne < deployWait) return keys;
        return (prevKeys & J) != 0 ? keys & ~J : keys | J;
    }

    /** Segments of 1-30 ticks of one key set from {@code options}. */
    final class Keys {
        private final int[] options;
        private int cur, left;

        Keys(int... options) {
            this.options = options;
        }

        int step() {
            if (left-- <= 0) {
                cur = options[r.nextInt(options.length)];
                left = 1 + r.nextInt(30);
            }
            return cur;
        }
    }

    // --- building ---------------------------------------------------------------------

    static final String[] SOLID = {"minecraft:stone", "minecraft:glass", "minecraft:oak_planks", "minecraft:oak_leaves[persistent=true]"};
    static final String[] FACINGS = {"north", "south", "east", "west"};

    /** Blocks with partial or odd collision shapes; none climbable, fluid, bouncy or sticky. */
    String odd() {
        return switch (r.nextInt(20)) {
            case 0 -> "minecraft:stone_slab[type=bottom]";
            case 1 -> "minecraft:stone_slab[type=top]";
            case 2, 3 -> "minecraft:oak_stairs[facing=" + FACINGS[r.nextInt(4)] + ",half=" + (r.nextBoolean() ? "top" : "bottom") + "]";
            case 4 -> "minecraft:oak_fence";
            case 5 -> "minecraft:cobblestone_wall";
            case 6 -> "minecraft:glass_pane";
            case 7 -> "minecraft:iron_bars";
            case 8 -> "minecraft:oak_trapdoor[facing=" + FACINGS[r.nextInt(4)] + ",half=" + (r.nextBoolean() ? "top" : "bottom") + "]";
            case 9 -> "minecraft:hopper";
            case 10 -> "minecraft:anvil[facing=" + FACINGS[r.nextInt(4)] + "]";
            case 11 -> "minecraft:cauldron";
            case 12 -> "minecraft:composter";
            case 13 -> "minecraft:lectern[facing=" + FACINGS[r.nextInt(4)] + "]";
            case 14 -> "minecraft:enchanting_table";
            case 15 -> "minecraft:heavy_core";
            case 16 -> "minecraft:stonecutter";
            case 17 -> "minecraft:dirt_path";
            case 18 -> "minecraft:iron_chain[axis=" + (r.nextBoolean() ? "x" : r.nextBoolean() ? "y" : "z") + "]";
            default -> "minecraft:oak_fence_gate[facing=" + FACINGS[r.nextInt(4)] + "]";
        };
    }

    /** Things that sit on a floor: step-ups and bumps. */
    String floorThing() {
        return switch (r.nextInt(12)) {
            case 0, 1 -> "minecraft:stone_slab[type=bottom]";
            case 2 -> "minecraft:oak_stairs[facing=" + FACINGS[r.nextInt(4)] + ",half=bottom]";
            case 3 -> "minecraft:snow[layers=" + (1 + r.nextInt(8)) + "]";
            case 4 -> "minecraft:white_carpet";
            case 5 -> "minecraft:dirt_path";
            case 6 -> "minecraft:heavy_core";
            case 7 -> "minecraft:oak_trapdoor[facing=north,half=bottom]";
            case 8 -> "minecraft:stone";
            case 9 -> "minecraft:soul_sand";
            default -> odd();
        };
    }

    void put(int x, int y, int z, String state) {
        blocks.put(new BlockPos(x, y, z), state);
    }

    /** A box of blocks, inclusive; each cell solid, or with chance {@code odd} an odd block. */
    void box(int x0, int y0, int z0, int x1, int y1, int z1, String solid, double odd) {
        for (int x = Math.min(x0, x1); x <= Math.max(x0, x1); x++)
            for (int y = Math.min(y0, y1); y <= Math.max(y0, y1); y++)
                for (int z = Math.min(z0, z1); z <= Math.max(z0, z1); z++)
                    put(x, y, z, r.nextDouble() < odd ? odd() : solid);
    }

    String solid() {
        return SOLID[r.nextInt(SOLID.length)];
    }

    /** A floor at y with things scattered on top, half extent {@code h} around the origin. */
    void floor(int y, int h, String base, int things) {
        box(origin.getX() - h, y, origin.getZ() - h, origin.getX() + h, y, origin.getZ() + h, base, 0.0);
        for (int i = 0; i < things; i++) {
            int x = origin.getX() - h + r.nextInt(2 * h + 1), z = origin.getZ() - h + r.nextInt(2 * h + 1);
            if (Math.abs(x - origin.getX()) <= 1 && Math.abs(z - origin.getZ()) <= 1) continue; // keep the start clear
            if (r.nextInt(5) == 0) {
                put(x, y, z, r.nextBoolean() ? "minecraft:ice" : "minecraft:soul_sand");
            } else {
                put(x, y + 1, z, floorThing());
            }
        }
    }

    // --- kinds ------------------------------------------------------------------------

    /** Open air: deploy from a standstill, then random rotations and keys. */
    static final class Air extends Scenario {
        private final Keys keys = new Keys(0, 0, W, J, S, W | A, W | SPRINT, D);

        Air(long seed) {
            super("air", seed, 260);
            length = 300;
            minY = 40;
        }

        @Override
        void build() {
            platform = origin.below();
        }

        @Override
        void aim(LocalPlayer p, int i) {
            // pull up when low; why? the case should stay airborne for its length
            if (p.getY() < origin.getY() - 150) look.range(-90.0F, -5.0F);
            else look.range(-90.0F, 90.0F);
        }

        @Override
        int keys(LocalPlayer p, int i) {
            return keys.step();
        }
    }

    /** A wall, an inside corner, a room or a ceiling ahead, with a catch floor below. */
    static final class Walls extends Scenario {
        private final Keys keys = new Keys(0, 0, W, J, W | SPRINT, A, S);
        private int approach;
        private boolean hit;
        String structure;

        Walls(long seed) {
            super("walls", seed, 200);
            length = 300;
            minY = origin.getY() - 70;
        }

        @Override
        void build() {
            platform = origin.below();
            double dx = -Math.sin(Math.toRadians(startYaw)), dz = Math.cos(Math.toRadians(startYaw));
            boolean alongX = Math.abs(dx) > Math.abs(dz);
            int sa = (int) Math.signum(alongX ? dx : dz), sb = (int) Math.signum(alongX ? dz : dx);
            if (sb == 0) sb = 1;
            int ox = origin.getX(), oy = origin.getY(), oz = origin.getZ();
            double odd = r.nextDouble() * 0.4;
            String solid = solid();
            approach = 20 + r.nextInt(60);
            int dist = 25 + r.nextInt(40);
            // by seed, so any 4 consecutive seeds cover all four
            switch ((int) (seed % 4)) {
                case 0 -> {
                    structure = "wall";
                    plane(alongX, ox, oz, sa * dist, -40, 40, oy - 60, oy + 8, solid, odd);
                }
                case 1 -> {
                    structure = "corner";
                    plane(alongX, ox, oz, sa * dist, -40 * sb, 45 * sb, oy - 60, oy + 8, solid, odd);
                    plane(!alongX, ox, oz, sb * (8 + r.nextInt(25)), -10 * sa, sa * (dist + 1), oy - 60, oy + 8, solid, odd);
                }
                case 2 -> {
                    structure = "room";
                    int hx = 6 + r.nextInt(20), hz = 6 + r.nextInt(20), h = 6 + r.nextInt(40);
                    int top = oy + 2 + r.nextInt(3), bottom = oy - h;
                    box(ox - hx, bottom, oz - hz, ox + hx, bottom, oz + hz, solid, odd);
                    box(ox - hx, top, oz - hz, ox + hx, top, oz + hz, solid, odd);
                    box(ox - hx, bottom, oz - hz, ox - hx, top, oz + hz, solid, odd);
                    box(ox + hx, bottom, oz - hz, ox + hx, top, oz + hz, solid, odd);
                    box(ox - hx, bottom, oz - hz, ox + hx, top, oz - hz, solid, odd);
                    box(ox - hx, bottom, oz + hz, ox + hx, top, oz + hz, solid, odd);
                    for (int n = r.nextInt(8); n > 0; n--) {
                        int px = ox - hx + 2 + r.nextInt(2 * hx - 3), pz = oz - hz + 2 + r.nextInt(2 * hz - 3);
                        if (Math.abs(px - ox) <= 2 && Math.abs(pz - oz) <= 2) continue;
                        box(px, bottom, pz, px + r.nextInt(2), top, pz + r.nextInt(2), solid(), odd);
                    }
                    approach = 0;
                }
                default -> {
                    structure = "ceiling";
                    int y = oy - 15 - r.nextInt(20);
                    int a0 = sa * (10 + r.nextInt(15)), a1 = a0 + sa * 60;
                    for (int a = Math.min(a0, a1); a <= Math.max(a0, a1); a++)
                        for (int b = -30; b <= 30; b++)
                            put(alongX ? ox + a : ox + b, y, alongX ? oz + b : oz + a, r.nextDouble() < odd ? odd() : solid);
                }
            }
            // the catch floor, under everything
            box(ox - 48, oy - 62, oz - 48, ox + 48, oy - 62, oz + 48, "minecraft:stone", 0.0);
        }

        /** A vertical plane at {@code a} along the main axis, {@code b0..b1} across it. */
        private void plane(boolean alongX, int ox, int oz, int a, int b0, int b1, int y0, int y1, String solid, double odd) {
            if (alongX) box(ox + a, y0, oz + b0, ox + a, y1, oz + b1, solid, odd);
            else box(ox + b0, y0, oz + a, ox + b1, y1, oz + a, solid, odd);
        }

        @Override
        void aim(LocalPlayer p, int i) {
            hit |= p.horizontalCollision || p.verticalCollision && !p.onGround();
            if (!hit && i < approach) {
                // head for the structure: a dive to build speed, little turning
                look.yaw = startYaw + (float) (r.nextGaussian() * 2.0);
                look.range(5.0F, 45.0F);
            } else {
                look.range(-90.0F, 90.0F);
            }
        }

        @Override
        int keys(LocalPlayer p, int i) {
            return keys.step();
        }
    }

    /** Walking, sprinting, jumps, step-ups and glide starts off a raised floor. */
    static final class Ground extends Scenario {
        private final Keys keys = new Keys(W, W, W | SPRINT, W | SPRINT, W | A, W | D, W | SPRINT | J, A, D, S, 0, W | SHIFT, J);

        Ground(long seed) {
            super("ground", seed, 100);
            length = 400;
            minY = origin.getY() - 20;
        }

        @Override
        void build() {
            int oy = origin.getY();
            floor(oy - 1, 24, r.nextBoolean() ? "minecraft:stone" : "minecraft:grass_block", 60 + r.nextInt(100));
            // rows of bottom slabs to step onto
            for (int n = 2 + r.nextInt(4); n > 0; n--) {
                int x = origin.getX() - 20 + r.nextInt(41), z = origin.getZ() - 20 + r.nextInt(41), len = 3 + r.nextInt(8);
                boolean alongX = r.nextBoolean();
                for (int k = 0; k < len; k++) {
                    int bx = alongX ? x + k : x, bz = alongX ? z : z + k;
                    if (Math.abs(bx - origin.getX()) > 1 || Math.abs(bz - origin.getZ()) > 1) put(bx, oy, bz, "minecraft:stone_slab[type=bottom]");
                }
            }
            // a low roof to bump into when jumping: its underside 2-2.5 blocks up
            int rx = origin.getX() - 3 - r.nextInt(12), rz = origin.getZ() - 3 - r.nextInt(12);
            String roof = r.nextBoolean() ? "minecraft:stone" : "minecraft:stone_slab[type=top]";
            box(rx - 7, oy + 2, rz - 7, rx, oy + 2, rz, roof, 0.2);
            // a raised block with a slab-and-stair way up
            // clear of the start: the block spans px..px+5, its way up px-4..px-1
            int px = origin.getX() + (r.nextBoolean() ? 6 + r.nextInt(10) : -8 - r.nextInt(10));
            int pz = origin.getZ() + (r.nextBoolean() ? 3 + r.nextInt(10) : -8 - r.nextInt(10));
            int ph = 1 + r.nextInt(4);
            box(px, oy, pz, px + 5, oy + ph - 1, pz + 5, "minecraft:stone", 0.0);
            for (int s = 0; s < ph; s++) {
                put(px - 1 - s, oy + ph - 1 - s, pz + 2, "minecraft:stone_slab[type=bottom]");
                if (ph - 1 - s > 0) box(px - 1 - s, oy, pz + 2, px - 1 - s, oy + ph - 2 - s, pz + 2, "minecraft:stone", 0.0);
                put(px - 1 - s, oy + ph - 1 - s, pz + 3, "minecraft:oak_stairs[facing=east,half=bottom]");
                if (ph - 1 - s > 0) box(px - 1 - s, oy, pz + 3, px - 1 - s, oy + ph - 2 - s, pz + 3, "minecraft:stone", 0.0);
            }
            // the floor's edge drops to a lower one
            box(origin.getX() - 48, oy - 13, origin.getZ() - 48, origin.getX() + 48, oy - 13, origin.getZ() + 48, "minecraft:stone", 0.0);
        }

        @Override
        double deployChance() {
            return 0.4;
        }

        @Override
        void aim(LocalPlayer p, int i) {
            if (p.isFallFlying()) look.range(-30.0F, 60.0F);
            else look.range(-40.0F, 70.0F);
        }

        @Override
        int keys(LocalPlayer p, int i) {
            return keys.step();
        }
    }

    /** Glide down onto a floor, then ebounces and walking. */
    static final class Landing extends Scenario {
        private final Keys ground = new Keys(W, W | SPRINT, W | SPRINT | J, J, 0, W | A, S);
        private final Keys air = new Keys(0, W, J, W | SPRINT);
        private final int dive;
        private boolean landed;

        Landing(long seed) {
            super("landing", seed, 100);
            length = 250;
            dive = 10 + r.nextInt(60);
            minY = 0; // set in build
        }

        @Override
        void build() {
            platform = origin.below();
            int h = 3 + r.nextInt(28);
            int fy = origin.getY() - h - 1;
            String base = new String[]{"minecraft:stone", "minecraft:grass_block", "minecraft:oak_planks", "minecraft:glass"}[r.nextInt(4)];
            floor(fy, 32, base, r.nextInt(300));
            minY = fy - 20;
        }

        @Override
        void aim(LocalPlayer p, int i) {
            landed |= p.onGround() && i > 2;
            if (!landed) look.range(dive - 10, dive + 10);
            else look.range(-40.0F, 80.0F);
        }

        @Override
        int keys(LocalPlayer p, int i) {
            return landed ? ground.step() : air.step();
        }
    }
}
