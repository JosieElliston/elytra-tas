//! The least ticks from a flight state to a point, velocity free: the stretch between
//! two boosters. why velocity free? a booster replaces the velocity, so what a
//! stretch into one has to deliver is a position (and the rotation it holds there),
//! not a state.
//!
//! DDP's `Goal::Reach` over every horizon from a guess down until one can't reach the
//! point (`burrowed_grove`'s `resolve` does the same against a recorded state), the
//! velocity's scale set so large that it carries no weight; then the faithful repair
//! (`refly::repair`) from the least horizon up until one flies clean.

use std::time::Instant;

use sim::generic::Generic;
use sim::state::Input;
use world::World;

use super::ddp::{Goal, Opts, Problem, Solved, U, X};
use super::refly::{Repaired, repair};
use super::stretch::resample;
use super::walls::Walls;

/// The velocity's scale in the goal: free.
pub const FREE: f64 = 1e8;
/// Horizons tried above the guess when the guess can't reach the point.
pub const ABOVE: usize = 25;
/// Horizons in a row that may fail below the least that reached, before the search
/// stops. why not stop at the first? a horizon's solve depends on its seed, and
/// one that fails can sit above one that solves (seen on alpine's last stretch).
pub const MISSES: usize = 2;

#[derive(Debug, Clone)]
pub struct Reached {
    /// The repaired rotations and the faithful states they fly (`x0` first).
    pub inputs: Vec<Input<f32>>,
    pub flown: Vec<X>,
    /// The least horizon the generic kernel reached the point in, and per horizon tried
    /// a note.
    pub generic: usize,
    pub tried: Vec<String>,
    pub secs: f64,
}

/// A closure's signature, spelled out: the problem borrows the walls it is given.
fn for_walls<F: for<'a> Fn(&'a Walls) -> Problem<'a>>(f: F) -> F {
    f
}

fn walls_for(world: &World, pts: &[[f64; 3]]) -> Walls {
    Walls::around(world, pts, 4.0, 1.0)
}

/// Solve `make(walls)` from `seed`; if the plan leaves the walls' region, grow it
/// around the plan and solve again.
fn solve_covered(
    world: &World,
    walls: &mut Walls,
    pts: &mut Vec<[f64; 3]>,
    make: &impl for<'a> Fn(&'a Walls) -> Problem<'a>,
    seed: &[U],
) -> Solved {
    loop {
        let s = make(walls).solve(seed);
        let out: Vec<[f64; 3]> = s
            .plan
            .x
            .iter()
            .map(|x| [x[0], x[1], x[2]])
            .filter(|p| !walls.covers(*p))
            .collect();
        if out.is_empty() {
            return s;
        }
        pts.extend(out);
        *walls = walls_for(world, pts);
    }
}

/// From `x0` (the rotation before it `yaw_from`, for the yaw's branch) to `target` in
/// as few ticks as the faithful kernel flies clean, with `seed` (its length the first
/// horizon tried) as the guess and `path` the points the walls are taken around.
/// `accept` judges a repaired flight (say, its end near enough the point); the least
/// horizon that is clean and accepted wins. Err when none up to the guess plus
/// [`ABOVE`] is.
pub fn to_point(
    world: &World,
    x0: X,
    yaw_from: f64,
    target: [f64; 3],
    seed: &[U],
    path: &[[f64; 3]],
    accept: impl Fn(&Repaired) -> bool,
) -> Result<Reached, String> {
    let t0 = Instant::now();
    let kernel = Generic::default();
    let goal = Goal::Reach {
        target: [target[0], target[1], target[2], 0.0, 0.0, 0.0],
        scale: [1.0, 1.0, 1.0, FREE, FREE, FREE],
    };
    let opts = Opts {
        tol: 1e-6,
        ..Opts::default()
    };
    let mut pts = path.to_vec();
    pts.push(target);
    let mut walls = walls_for(world, &pts);
    let problem = for_walls(|w| Problem {
        kernel,
        walls: w,
        x0,
        goal: goal.clone(),
        opts: opts.clone(),
    });
    let mut tried = Vec::new();
    let n0 = seed.len().max(1);
    // the generic kernel: up from the guess to the first horizon that reaches, then
    // down while one does
    let mut best: Option<Solved> = None;
    let mut u = seed.to_vec();
    for n in n0..=n0 + ABOVE {
        let s = solve_covered(world, &mut walls, &mut pts, &problem, &resample(&u, n));
        tried.push(format!("{n}:{}", if s.feasible { "ok" } else { "no" }));
        if s.feasible {
            best = Some(s);
            break;
        }
        u = s.plan.u.clone();
    }
    let Some(mut best) = best else {
        return Err(format!(
            "no horizon reaches the point ({})",
            tried.join(" ")
        ));
    };
    let mut n = best.plan.u.len();
    let mut misses = 0;
    while n > 1 && misses < MISSES {
        n -= 1;
        let s = solve_covered(
            world,
            &mut walls,
            &mut pts,
            &problem,
            &resample(&best.plan.u, n),
        );
        tried.push(format!("{n}:{}", if s.feasible { "ok" } else { "no" }));
        if s.feasible {
            best = s;
            misses = 0;
        } else {
            misses += 1;
        }
    }
    let generic = best.plan.u.len();
    // the faithful kernel: the least horizon whose repair flies clean
    let mut used = best;
    loop {
        let pb = problem(&walls);
        let rep = repair(world, &pb, &used, yaw_from, 100.0, 3);
        let n = used.plan.u.len();
        let ok = rep.clean() && rep.inputs.len() == n && accept(&rep);
        tried.push(format!("faithful {n}:{}", if ok { "ok" } else { "no" }));
        if ok {
            return Ok(Reached {
                inputs: rep.inputs,
                flown: rep.flown,
                generic,
                tried,
                secs: t0.elapsed().as_secs_f64(),
            });
        }
        if n >= generic + ABOVE {
            return Err(format!(
                "no repaired horizon flies clean ({})",
                tried.join(" ")
            ));
        }
        used = pb.solve(&resample(&used.plan.u, n + 1));
    }
}
