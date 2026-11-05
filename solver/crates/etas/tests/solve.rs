//! The first solver on a toy course (open space and one wall beside the start, with a
//! goal plane slanted toward the wall, so the fastest way there leans on it), and the
//! swept-leg distance its wall constraints use.

use etas::solve::ddp::{Goal, Multipliers, Opts, Problem, Rate, U, X, control, server_hits};
use etas::solve::refly::refly;
use etas::solve::walls::Walls;
use sim::generic::Generic;
use world::{BlockPos, World};

/// Air over x -10..10, y 30..115, z -8..75; a stone wall at x = 3 (its face at 3.0),
/// z -5..70, y 40..110. Start at (0.5, 100, 0.5) gliding +z at 1 b/t.
fn toy() -> (World, X, Goal) {
    let mut w = World::air(
        BlockPos {
            x: -10,
            y: 30,
            z: -8,
        },
        BlockPos {
            x: 10,
            y: 115,
            z: 75,
        },
    );
    for y in 40..=110 {
        for z in -5..=70 {
            w.set(BlockPos { x: 3, y, z }, "minecraft:stone");
        }
    }
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let goal = Goal::Plane {
        normal: [s, 0.0, s],
        at: (0.5 + 40.0) * s,
        cap: 150,
    };
    (w, [0.5, 100.0, 0.5, 0.0, 0.0, 1.0], goal)
}

fn walls(w: &World) -> Walls {
    Walls::region(w, [-9, 31, -7], [9, 114, 74], 1.0)
}

/// The straight-line seed: no lead, pitch 0, along +z.
fn straight(n: usize) -> Vec<U> {
    vec![control(0.0, 0.0); n]
}

#[test]
fn beats_straight_seed_and_clears_the_wall_under_faithful() {
    let (w, x0, goal) = toy();
    let walls = walls(&w);
    let pb = Problem {
        kernel: Generic::default(),
        walls: &walls,
        x0,
        goal,
        opts: Opts::default(),
    };
    // the seed, cut to its crossing
    let mut m = Multipliers::new(1.0, 0);
    let mut u = straight(60);
    let xs = pb.refit(&mut m, &mut u);
    let seed_time = pb.value(&xs);
    assert!(pb.feasible(&etas::solve::ddp::Plan {
        u: u.clone(),
        x: xs.clone()
    }));

    let t = std::time::Instant::now();
    let s = pb.solve(&u);
    let secs = t.elapsed().as_secs_f64();
    let time = pb.value(&s.plan.x);
    let worst = pb.worst(&s.plan.x, &s.plan.u);
    println!(
        "seed {seed_time:.4} ticks, solved {time:.4} in {secs:.2} s, worst clearance {worst:.4}, stages {:?}",
        s.stages
    );
    assert!(s.feasible, "{:?}", s.stages);
    assert!(time < seed_time - 1.0, "solved {time} vs seed {seed_time}");
    // the wall is in the way: the solution leans on it
    assert!(worst < 0.03, "wall inactive (worst clearance {worst})");

    // faithful re-flight of the generic rotations: no contact, the same crossing
    let inputs = s.plan.inputs(0.0);
    let p = refly(&w, &x0, &s.plan.x, &inputs, 1e-3);
    println!(
        "faithful: max gap {:.2e}, contacts {:?}, event {:?}",
        p.max_gap(),
        p.contacts,
        p.event
    );
    assert!(p.clean(), "contacts {:?} event {:?}", p.contacts, p.event);
    // the generic kernel is ~1e-5 a tick off the game, and open loop that compounds
    assert!(p.max_gap() < 1e-2);
    let ft = pb.value(&p.flown);
    println!("faithful crossing {ft:.4}");
    assert!((ft - time).abs() < 1e-2, "faithful {ft} vs generic {time}");
}

#[test]
fn the_servers_box_is_where_the_velocity_would_carry_it() {
    let (w, ..) = toy();
    let walls = walls(&w);
    // the wall's face is at 3.0 and the box reaches 0.3 past the feet
    let hit = [2.0, 100.0, 10.0, 0.9, 0.0, 0.0];
    let clear = [2.0, 100.0, 10.0, 0.5, 0.0, 0.0];
    // on its own velocity, then on the same velocity a tick behind
    let hits = server_hits(&walls, &[clear, hit, clear]);
    assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), [1, 2]);
    for h in &hits {
        assert!((h.1 + 0.2).abs() < 1e-6, "{hits:?}");
    }
}

/// A control schedule near the wall, away from the kernel's kinks.
fn wavy(n: usize) -> Vec<U> {
    (0..n)
        .map(|k| {
            let t = k as f64;
            control(-0.35 + 0.1 * (0.3 * t).sin(), 20.0 + 15.0 * (0.2 * t).cos())
        })
        .collect()
}

#[test]
fn gradient_through_a_rollout_matches_finite_differences() {
    let (w, x0, plane) = toy();
    let walls = walls(&w);
    let n = 30;
    let mut pb = Problem {
        kernel: Generic::default(),
        walls: &walls,
        x0,
        goal: plane,
        opts: Opts {
            margin: 0.5,
            ..Opts::default()
        },
    };
    let u = wavy(n);
    let x = pb.rollout(&u);
    let target = x[n];
    let mut m = Multipliers::new(10.0, n);
    for (k, l) in m.walls.iter_mut().enumerate() {
        *l = std::array::from_fn(|p| 0.01 * ((k + p) % 3) as f64);
    }
    m.goal = [0.1, -0.2, 0.05, 0.3, -0.1, 0.2];
    // a perturbed target, so the reach goal's error isn't 0
    let reach = Goal::Reach {
        target: std::array::from_fn(|i| target[i] + 0.3 * (i as f64 - 2.5)),
        scale: [1.0, 1.0, 1.0, 0.05, 0.05, 0.05],
    };
    let plane = pb.goal.clone();
    let rates = [
        (plane.clone(), Rate::default(), false),
        (reach.clone(), Rate::default(), false),
        (
            plane,
            Rate {
                weight: 2.0,
                delta: 0.05,
                from: Some([0.3, -0.2]),
                to: Some([-0.1, 0.4]),
            },
            false,
        ),
        (reach, Rate::default(), true),
    ];
    for (goal, rate, server) in rates {
        pb.goal = goal;
        pb.opts.rate = rate;
        pb.opts.server = server;
        let (j, g) = pb.gradient(&m, &u);
        let active = (0..n).any(|k| pb.worst(&x[k..k + 2], &u[k..k + 1]) < pb.opts.margin);
        assert!(active, "the walls should be active somewhere");
        let cost = |u: &[U]| pb.cost(&m, &pb.rollout(u), u);
        assert_eq!(j, cost(&u));
        let h = 1e-6;
        let mut worst: f64 = 0.0;
        for k in 0..n {
            for c in 0..2 {
                let (mut up, mut um) = (u.clone(), u.clone());
                up[k][c] += h;
                um[k][c] -= h;
                let fd = (cost(&up) - cost(&um)) / (2.0 * h);
                let err = (g[k][c] - fd).abs() / (1.0 + fd.abs());
                worst = worst.max(err);
                assert!(
                    err < 1e-5,
                    "{:?} tick {k} control {c}: adjoint {} fd {fd}",
                    pb.goal,
                    g[k][c]
                );
            }
        }
        println!("{:?}: worst relative error {worst:.2e}", pb.goal);
    }
}

#[test]
fn same_input_same_output() {
    let (w, x0, goal) = toy();
    let walls = walls(&w);
    let pb = Problem {
        kernel: Generic::default(),
        walls: &walls,
        x0,
        goal,
        opts: Opts::default(),
    };
    let a = pb.solve(&straight(60));
    let b = pb.solve(&straight(60));
    let bits = |u: &[U]| {
        u.iter()
            .flat_map(|c| c.map(f64::to_bits))
            .collect::<Vec<_>>()
    };
    assert_eq!(bits(&a.plan.u), bits(&b.plan.u));
    assert_eq!(a.stages, b.stages);
    assert_eq!(a.mult, b.mult);
}

#[test]
fn leg_sdf_is_the_swept_box() {
    use sim::generic::ghost::{box_sdf, leg_sdf};
    use world::Aabb;
    // a small LCG: deterministic, no dev-dependency
    let mut s: u64 = 0x2545F4914F6CDD1D;
    let mut r = |lo: f64, hi: f64| {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        lo + (hi - lo) * ((s >> 11) as f64 / (1u64 << 53) as f64)
    };
    let b = Aabb {
        min: [0.0, 0.0, 0.0],
        max: [1.0, 0.25, 1.0],
    };
    for _ in 0..2000 {
        let p: [f64; 3] = std::array::from_fn(|_| r(-2.5, 2.5));
        assert_eq!(leg_sdf(p, p, &b), box_sdf(p, &b));
        let axis = (r(0.0, 3.0) as usize).min(2);
        let mut q = p;
        q[axis] += r(-3.0, 3.0);
        let leg = leg_sdf(p, q, &b);
        // the least distance over dense points of the leg, from above
        let dense = (0..=2000)
            .map(|j| {
                let t = j as f64 / 2000.0;
                box_sdf(std::array::from_fn(|i| p[i] + t * (q[i] - p[i])), &b)
            })
            .fold(f64::INFINITY, f64::min);
        if leg > 0.0 {
            assert!(
                dense >= leg - 1e-12 && dense - leg < 2e-3,
                "leg {leg} dense {dense}"
            );
        } else {
            assert!(dense <= 1.5e-3, "leg {leg} dense {dense}");
        }
    }
}
