//! Generic kernel: dual numbers against finite differences, the ablation hook at its
//! identity, and the ghost geometry.

use nalgebra::SVector;
use num_dual::{Dual32, Dual64, HyperDual64};
use proptest::prelude::*;
use sim::generic::ghost::{HALF_WIDTH, HEIGHT, box_sdf, l_move, sdf};
use sim::faithful::Faithful;
use sim::generic::{GRAVITY, Generic, Hooks, LiftShape, PITCH_MAX, PITCH_MIN, Scalar};
use sim::state::{FlightState, Input};
use world::Aabb;

/// The kernel step as a function of 8 numbers: pos, vel, yaw, pitch -> pos, vel.
fn f<T: Scalar>(k: &Generic, x: [T; 8]) -> [T; 6] {
    let s = FlightState {
        pos: [x[0], x[1], x[2]],
        vel: [x[3], x[4], x[5]],
    };
    let n = k.step(
        &s,
        &Input {
            yaw: x[6],
            pitch: x[7],
        },
    );
    [n.pos[0], n.pos[1], n.pos[2], n.vel[0], n.vel[1], n.vel[2]]
}

fn f64_at(k: &Generic, x: [f64; 8]) -> [f64; 6] {
    f(k, x)
}

/// A random point away from every branch switch (see the generic module's list), and
/// with horizontal speed over 0.1. why? the step's curvature grows like an inverse
/// power of the horizontal speed, so at 0.012 the 1e-4 finite-difference step is off
/// by 6e-5 relative while the hyperdual value is right (the differences converge to it
/// as the step shrinks).
fn smooth_point() -> impl Strategy<Value = [f64; 8]> {
    (
        prop::array::uniform3(-200.0..200.0f64),
        prop::array::uniform3(-3.0..3.0f64),
        -720.0..720.0f64,
        PITCH_MIN..=PITCH_MAX,
    )
        .prop_map(|(p, v, yaw, pitch)| [p[0], p[1], p[2], v[0], v[1], v[2], yaw, pitch])
        .prop_filter("away from branch switches", |x| {
            let lift = (x[7].to_radians().cos()).powi(2);
            let vy_g = x[4] + GRAVITY * (-1.0 + 0.75 * lift);
            x[7].abs() > 0.01 && vy_g.abs() > 1e-3 && x[4].abs() > 0.01 && x[3].hypot(x[5]) > 0.1
        })
}

proptest! {
    #[test]
    fn gradient_matches_central_differences(x in smooth_point()) {
        let k = Generic::default();
        for i in 0..8 {
            let d: [Dual64; 8] = std::array::from_fn(|j| {
                let v = Dual64::from_re(x[j]);
                if j == i { v.derivative() } else { v }
            });
            let ad = f(&k, d);
            let h = 1e-6;
            let (mut xp, mut xm) = (x, x);
            xp[i] += h;
            xm[i] -= h;
            let (fp, fm) = (f64_at(&k, xp), f64_at(&k, xm));
            for o in 0..6 {
                let fd = (fp[o] - fm[o]) / (2.0 * h);
                // FD error here is ~eps|f|/h ~ 1e-8 for positions near 200.
                prop_assert!((ad[o].eps - fd).abs() < 1e-6 * (1.0 + fd.abs()),
                    "d out{o}/d in{i}: dual {} fd {fd}", ad[o].eps);
                // why not exact? a dual's sin and cos and f64's sin_cos can differ by an ulp
                let plain = f64_at(&k, x)[o];
                prop_assert!((ad[o].re - plain).abs() < 1e-12 * (1.0 + plain.abs()));
            }
        }
    }

    #[test]
    fn hessian_matches_central_differences(x in smooth_point()) {
        // Velocity outputs over the velocity and rotation inputs: position enters linearly.
        let k = Generic::default();
        for i in 3..8 {
            for j in i..8 {
                let hd: [HyperDual64; 8] = std::array::from_fn(|m| {
                    let mut v = HyperDual64::from_re(x[m]);
                    if m == i { v = v.derivative1(); }
                    if m == j { v = v.derivative2(); }
                    v
                });
                let ad = f(&k, hd);
                let h = 1e-4;
                let at = |si: f64, sj: f64| {
                    let mut y = x;
                    y[i] += si * h;
                    y[j] += sj * h;
                    f64_at(&k, y)
                };
                let (pp, pm, mp, mm) = (at(1.0, 1.0), at(1.0, -1.0), at(-1.0, 1.0), at(-1.0, -1.0));
                for o in 3..6 {
                    let fd = (pp[o] - pm[o] - mp[o] + mm[o]) / (4.0 * h * h);
                    prop_assert!((ad[o].eps1eps2 - fd).abs() < 1e-5 * (1.0 + fd.abs()),
                        "d2 out{o}/d in{i} d in{j}: hyperdual {} fd {fd}", ad[o].eps1eps2);
                }
            }
        }
    }

    #[test]
    fn lift_pow_one_is_vanilla(x in prop::array::uniform8(-3.0..3.0f64), yaw in -720.0..720.0f64, pitch in -90.0..=90.0f64) {
        let vanilla = Generic::default();
        let pow1 = Generic { hooks: Hooks { lift_shape: LiftShape::Pow(1.0) }, ..vanilla };
        let mut y = x;
        y[6] = yaw;
        y[7] = pitch;
        let (a, b) = (f64_at(&vanilla, y), f64_at(&pow1, y));
        prop_assert_eq!(a.map(f64::to_bits), b.map(f64::to_bits));
        let d: [Dual64; 8] = std::array::from_fn(|j| Dual64::from_re(y[j]).derivative());
        let (a, b) = (f(&vanilla, d), f(&pow1, d));
        prop_assert_eq!(a.map(|v| v.eps.to_bits()), b.map(|v| v.eps.to_bits()));
    }

    #[test]
    fn sdf_sign_matches_overlap(p in prop::array::uniform3(-3.0..3.0f64),
                                lo in prop::array::uniform3(-2.0..2.0f64),
                                ext in prop::array::uniform3(0.01..2.0f64)) {
        let b = Aabb { min: lo, max: [lo[0] + ext[0], lo[1] + ext[1], lo[2] + ext[2]] };
        let me = Aabb {
            min: [p[0] - HALF_WIDTH, p[1], p[2] - HALF_WIDTH],
            max: [p[0] + HALF_WIDTH, p[1] + HEIGHT, p[2] + HALF_WIDTH],
        };
        let overlap = (0..3).all(|i| me.min[i] < b.max[i] && b.min[i] < me.max[i]);
        let d = box_sdf(p, &b);
        prop_assert_eq!(d < 0.0, overlap, "d {}", d);
        // The gradient is finite and, away from inside kinks, unit length.
        let g: [f64; 3] = std::array::from_fn(|i| {
            let q: [Dual64; 3] = std::array::from_fn(|j| {
                let v = Dual64::from_re(p[j]);
                if i == j { v.derivative() } else { v }
            });
            box_sdf(q, &b).eps
        });
        prop_assert!(g.iter().all(|x| x.is_finite()));
        if d > 0.0 {
            let n = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
            prop_assert!((n - 1.0).abs() < 1e-9, "|grad| {}", n);
        }
    }
}

#[test]
fn sdf_simple_boxes() {
    let at0 = [0.0, 0.0, 0.0];
    let unit = |x: f64, y: f64, z: f64| Aabb {
        min: [x, y, z],
        max: [x + 1.0, y + 1.0, z + 1.0],
    };
    // Face: a block to +x, its face 0.7 from the box's +x side.
    assert!((box_sdf(at0, &unit(1.0, 0.0, -0.5)) - (1.0 - HALF_WIDTH)).abs() < 1e-15);
    // Touching: exactly 0. Below the feet, and to the -z side.
    assert_eq!(box_sdf(at0, &unit(-0.5, -1.0, -0.5)), 0.0);
    assert_eq!(box_sdf(at0, &unit(-0.5, 0.0, -1.0 - HALF_WIDTH)), 0.0);
    // Edge: 0.3 apart in x and 0.4 in y (above the head) -> 0.5.
    let d = box_sdf(at0, &unit(HALF_WIDTH + 0.3, HEIGHT + 0.4, -0.5));
    assert!((d - 0.5).abs() < 1e-12, "{d}");
    // Inside: box y 0.1..0.7 in a block's y 0..1; the shallowest way out is down by 0.7
    // (sideways would take 0.8).
    let d = box_sdf([0.5, 0.1, 0.5], &unit(0.0, 0.0, 0.0));
    assert!((d - -(0.1 + HEIGHT)).abs() < 1e-12, "{d}");
    // A list: the nearest.
    let near = sdf(at0, &[unit(5.0, 0.0, 0.0), unit(1.0, 0.0, -0.5)]).unwrap();
    assert!((near - (1.0 - HALF_WIDTH)).abs() < 1e-15);
    assert!(sdf::<f64>(at0, &[]).is_none());
}

#[test]
fn l_move_order() {
    let p = [1.0, 2.0, 3.0];
    // |x| > |z|: y, then x, then z.
    assert_eq!(
        l_move(p, [0.5, -0.25, 0.125]),
        [[1.0, 1.75, 3.0], [1.5, 1.75, 3.0], [1.5, 1.75, 3.125]]
    );
    // |x| < |z|: y, then z.
    assert_eq!(l_move(p, [0.125, 0.25, -0.5])[1], [1.0, 2.25, 2.5]);
    // Tie: x first.
    assert_eq!(l_move(p, [0.5, 0.0, -0.5])[1], [1.5, 2.0, 3.0]);
    // The end is where the contact-free step lands.
    let k = Generic::default();
    let s = FlightState {
        pos: p,
        vel: [1.3, -0.4, 0.7],
    };
    let n = k.step(
        &s,
        &Input {
            yaw: 30.0,
            pitch: 10.0,
        },
    );
    assert_eq!(l_move(s.pos, n.vel)[2], n.pos);
}

#[test]
fn zeroing_is_opt_out() {
    let s = FlightState {
        pos: [0.0; 3],
        vel: [1.0, 0.002, 0.5],
    };
    let u = Input {
        yaw: 0.0,
        pitch: 0.0,
    };
    let on = Generic::default().step(&s, &u);
    let off = Generic {
        zero_small_velocity: false,
        ..Generic::default()
    }
    .step(&s, &u);
    // At pitch 0, zeroed: gravity relief leaves -0.02, the conversion gives back a tenth.
    assert!((on.vel[1] - -0.018 * 0.98_f32 as f64).abs() < 1e-12);
    assert!((on.vel[1] - off.vel[1]).abs() > 1e-3);
    // A zeroed velocity differentiates to 0 without NaN.
    let d = Dual64::from_re(1e-3).derivative();
    let sd = FlightState {
        pos: [Dual64::from_re(0.0); 3],
        vel: [d, d, d],
    };
    let n = Generic::default().step(
        &sd,
        &Input {
            yaw: Dual64::from_re(0.0),
            pitch: Dual64::from_re(-10.0),
        },
    );
    assert!(n.vel.iter().all(|v| v.eps == 0.0));
}

#[test]
fn pitch_minus_90_is_dead() {
    // The game skips conversion, nose-up and turning at -90; +90 turns.
    let k = Generic::default();
    let s = FlightState {
        pos: [0.0; 3],
        vel: [1.0, -0.5, 0.0],
    };
    let dead = k.step(
        &s,
        &Input {
            yaw: 0.0,
            pitch: -90.0,
        },
    );
    assert_eq!(dead.vel[0], 1.0 * 0.99_f32 as f64);
    let down = k.step(
        &s,
        &Input {
            yaw: 0.0,
            pitch: 90.0,
        },
    );
    assert!(down.vel[2] > 0.09, "turns toward +z: {:?}", down.vel);
}

/// Both ends of the solver's pitch range are live in the game's kernel, the band below
/// `PITCH_MIN` is dead, and the two kernels agree up to the ends. why? a solver is told it
/// can use the whole range, and a dead tick there would be a cliff it can't see.
#[test]
fn pitch_range_is_live_and_tight() {
    let k = Faithful;
    let s = [1.0, -0.5, 0.0];
    let moved = |pitch: f32, yaw: f32| {
        let (v, _) = k.velocity(s, &k.look(&Input { yaw, pitch }));
        // the turn toward the look, which a dead tick skips
        (v[0] - 0.99_f32 as f64).abs() > 1e-6 || v[2].abs() > 1e-6
    };
    for yaw in [10.0, 100.0, -170.0, 1234.5] {
        assert!(moved(PITCH_MIN as f32, yaw), "PITCH_MIN dead at yaw {yaw}");
        assert!(moved(PITCH_MAX as f32, yaw), "PITCH_MAX dead at yaw {yaw}");
        assert!(!moved((PITCH_MIN - 2e-5) as f32, yaw), "the band ends lower");
        assert!(!moved(-90.0, yaw), "-90 is live");
    }
    let g = Generic::default();
    for pitch in [PITCH_MIN, -89.9, -89.0, 89.0, 89.9, 89.999, PITCH_MAX] {
        for yaw in [10.0f32, 100.0, -170.0] {
            let u = Input {
                yaw,
                pitch: pitch as f32,
            };
            let (a, _) = k.velocity(s, &k.look(&u));
            let b = g.velocity(
                s,
                &g.look(&Input {
                    yaw: yaw as f64,
                    pitch,
                }),
            );
            for i in 0..3 {
                assert!(
                    (a[i] - b[i]).abs() < 1e-3,
                    "pitch {pitch} yaw {yaw}: {a:?} {b:?}"
                );
            }
        }
    }
}

#[test]
fn vector_duals_and_f32_duals_run() {
    // One pass of `num_dual::hessian` (Dual2Vec) gives the whole gradient and Hessian;
    // it agrees with per-entry Dual64 / HyperDual64.
    let k = Generic::default();
    let x = [10.0, 60.0, -5.0, 0.8, -0.6, 1.1, 37.0, 12.0];
    let vy = |v: SVector<_, 8>| f(&k, std::array::from_fn(|i| v[i]))[4];
    let (val, grad, hess) = num_dual::hessian(vy, &SVector::from(x));
    assert_eq!(val, f64_at(&k, x)[4]);
    for i in 0..8 {
        let d: [Dual64; 8] = std::array::from_fn(|j| {
            let v = Dual64::from_re(x[j]);
            if j == i { v.derivative() } else { v }
        });
        assert!((f(&k, d)[4].eps - grad[i]).abs() < 1e-14);
        for j in 0..8 {
            let h: [HyperDual64; 8] = std::array::from_fn(|m| {
                let mut v = HyperDual64::from_re(x[m]);
                if m == i {
                    v = v.derivative1();
                }
                if m == j {
                    v = v.derivative2();
                }
                v
            });
            assert!((f(&k, h)[4].eps1eps2 - hess[(i, j)]).abs() < 1e-14);
        }
    }
    let (_, jac) = num_dual::jacobian(
        |v: SVector<_, 8>| SVector::from(f(&k, std::array::from_fn(|i| v[i]))),
        &SVector::from(x),
    );
    assert!((jac[(4, 7)] - grad[7]).abs() < 1e-14);
    // f32 duals: the same derivative to f32 precision.
    let d32: [Dual32; 8] = std::array::from_fn(|j| {
        let v = Dual32::from_re(x[j] as f32);
        if j == 7 { v.derivative() } else { v }
    });
    assert!(((f(&k, d32)[4].eps as f64) - grad[7]).abs() < 1e-5 * (1.0 + grad[7].abs()));
}

mod relaxed {
    use sim::generic::ghost::{HALF_WIDTH, box_sdf, l_move, leg_sdf, relaxed_move, segment_sdf};
    use world::Aabb;

    fn cube() -> Aabb {
        Aabb { min: [0.0, 0.0, 0.0], max: [1.0, 1.0, 1.0] }
    }

    /// Off the axes, the least distance over many points along the segment.
    #[test]
    fn segment_sdf_is_the_least_over_the_segment() {
        let cases = [
            ([-2.0, 0.2, 1.8], [1.9, 0.2, 2.2]),
            ([-1.5, 0.3, 0.5], [0.5, 0.3, 2.5]),
            ([-3.0, -2.0, 2.0], [0.0, 3.0, 4.0]),
        ];
        for (p, q) in cases {
            let brute = (0..=20000)
                .map(|i| {
                    let t = i as f64 / 20000.0;
                    box_sdf::<f64>(std::array::from_fn(|k| p[k] + (q[k] - p[k]) * t), &cube())
                })
                .fold(f64::INFINITY, f64::min);
            // the grid can only sit above the least, by up to a step's worth at a kink inside
            let d = segment_sdf::<f64>(p, q, &cube());
            assert!(d <= brute + 1e-9 && brute - d < 1e-3, "{p:?} {q:?}: {d} {brute}");
        }
    }

    /// The chord past a corner clears where the sweep's bounding box says it doesn't.
    #[test]
    fn segment_sdf_sees_past_the_bounding_box() {
        let (p, q) = ([-2.0, 0.2, 1.0], [1.0, 0.2, 4.0]);
        assert!(leg_sdf::<f64>(p, q, &cube()) < 0.0);
        assert!(segment_sdf::<f64>(p, q, &cube()) > 0.0);
    }

    /// Overlapping, it's how far the whole segment must move, at least the deepest point's depth.
    #[test]
    fn segment_sdf_overlapping_is_the_segments_depth() {
        let (p, q) = ([-1.0, -2.0, -1.0], [2.0, 3.0, 3.0]);
        let deepest = (0..=20000)
            .map(|i| {
                let t = i as f64 / 20000.0;
                box_sdf::<f64>(std::array::from_fn(|k| p[k] + (q[k] - p[k]) * t), &cube())
            })
            .fold(f64::INFINITY, f64::min);
        let d = segment_sdf::<f64>(p, q, &cube());
        assert!(d < 0.0 && d <= deepest, "{d} {deepest}");
    }

    /// Deep inside the box, the derivatives are the depth's, where a held parameter's would not be.
    /// Off `from[i] == to[i]`, which is a kink.
    #[test]
    fn segment_sdf_derivative_matches_inside() {
        use sim::generic::num_dual::DualSVec64;
        let (p, q) = ([-1.0, 0.1, 0.4], [1.3, 0.35, 0.9]);
        let z: [DualSVec64<6>; 6] = std::array::from_fn(|i| DualSVec64::<6>::from_re(if i < 3 { p[i] } else { q[i - 3] }).derivative(i));
        let d = segment_sdf([z[0], z[1], z[2]], [z[3], z[4], z[5]], &cube());
        let g = d.eps.0.unwrap();
        for i in 0..6 {
            let h = 1e-6;
            let mut a = [p, q];
            a[i / 3][i % 3] += h;
            let up = segment_sdf::<f64>(a[0], a[1], &cube());
            a[i / 3][i % 3] -= 2.0 * h;
            let dn = segment_sdf::<f64>(a[0], a[1], &cube());
            assert!(((up - dn) / (2.0 * h) - g[i]).abs() < 1e-5, "{i}: {} {}", (up - dn) / (2.0 * h), g[i]);
        }
    }

    #[test]
    fn segment_sdf_on_an_axis_is_the_leg() {
        let (p, q) = ([-2.0, 0.2, 1.0 + HALF_WIDTH + 0.1], [3.0, 0.2, 1.0 + HALF_WIDTH + 0.1]);
        assert_eq!(segment_sdf::<f64>(p, q, &cube()), leg_sdf::<f64>(p, q, &cube()));
    }

    #[test]
    fn relaxed_move_ends_the_l_and_the_straight_line() {
        let (p, v) = ([1.0, 2.0, 3.0], [0.9, -0.3, 1.5]);
        assert_eq!(relaxed_move::<f64>(p, v, [0.0, 0.0]), l_move::<f64>(p, v));
        let [a, b, e] = relaxed_move::<f64>(p, v, [1.0, 1.0]);
        for k in 0..3 {
            assert!((a[k] - (p[k] + v[k] / 3.0)).abs() < 1e-12);
            assert!((b[k] - (p[k] + v[k] * 2.0 / 3.0)).abs() < 1e-12);
            assert!((e[k] - (p[k] + v[k])).abs() < 1e-12);
        }
        let [a, b, e] = relaxed_move::<f64>(p, v, [1.0, 0.0]);
        assert_eq!(a, [p[0], p[1] + v[1], p[2]]);
        assert!(b[1] == a[1] && (b[0] - (a[0] + e[0]) / 2.0).abs() < 1e-12 && (b[2] - (a[2] + e[2]) / 2.0).abs() < 1e-12);
    }
}
