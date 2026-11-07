//! Phase `optimize`: every completed run in `runs/flights/` optimized from the tick the
//! timer starts to the finish, objective `finish_gate_segments`, and flown with port and
//! verified. The prefix up to the timer start stays as recorded. Writes
//! `runs/optimize/NAME.traj.jsonl` and prints a line per run.
//!
//! On the generic kernel, all ghost walls:
//!
//! 1. A contact-free plan: the recording cut every `FIRST` ticks, each piece a reach from
//!    the recorded state to the next one. why not the recorded rotations whole? a seed with
//!    a wall hit or a server velocity flies off the recording from there on.
//! 2. Rounds of sliding windows: per window length in `WINDOWS`, the plan cut at that
//!    length (the cuts offset between passes), each piece flown to the plan's own state at
//!    its end in as few ticks as solve, in parallel; the last piece to the finish gate.
//! 3. After each round, the whole run against the gate, kept if better. why both? a window
//!    can only drop whole ticks and throws away what's left over; the whole solve moves the
//!    leftover along the run, but alone it stalls at a local optimum about 3 ticks in
//!    (academy's fastest seed, 2026-11-04 23:45 EST: 622 to 618.6).
//!
//! 4. [`carry`], for a window's fraction of a tick. why? a window of step 2 flies to the plan's own state a
//!    whole number of ticks on, so it keeps only whole ticks, and every flight ends the same fraction past a
//!    whole tick, its reference's (academy, 2026-11-06: N.08 for every N, from any start). Often it stops short:
//!    up to 0.49 ticks carried, then a window it can't hold, around tick 100, and a gate solve from there only
//!    rejoins the old tail. Best seen 0.08 ticks at the finish.
//!
//! Then `refly::track` flies the plan under the faithful kernel.
//!
//! With `--relax`, the seeds are `runs/optimize/`'s flights and the results go to
//! `runs/relax/`: the first plan, [`smooth`]ed at each price in `RELAX`, the rounds again, and
//! [`smooth`] once more at `FINAL`.
//!
//! With `--hybrid`, the same on `runs/hybrid/`: a flight with a stretch from another attempt, to
//! try its route.
//!
//! With `--plain`, those without any price on rotation, for what a seed is worth on time alone.
//!
//! With `--open-top`, the start area's top is its outer bound, not the highest anyone was seen
//! inside it, for `dome`'s flights from under the roof, above any recording.
//!
//! `--margin BLOCKS` sets the clearance from walls, `MARGIN` otherwise. It's the active constraint along a
//! flight: academy's 529s reach 528 at 0.012 (2026-11-06), since they were 0.01 blocks short of the finish.
//!
//! `--forbid X0 Y0 Z0 X1 Y1 Z1`, repeatable, adds an obstacle only the solve sees, to hold a flight to one route.
//! `verify` judges the result in the real world.
//!
//! What the objective doesn't see of the timer, which counts poses, not crossings (as of 2026-11-06 06:18 EST):
//! - The start is fixed at the first pose past the door, so how far past it is (`dome`'s `slabphase`) isn't solved for.
//! - The finish needs a block-changing move after the first pose past the plane. academy's wall at z -98.3 denies one
//!   when that pose is past z -99, so a crossing late in a tick reads 529 and verifies at 533 or never finishes.
//!
//! `--triple` asks whether chords get stuck less than L's, on `runs/flights/`'s runs, one at a time. An L chain's first plan
//! and rounds give checkpoints; from the recording and from each checkpoint, a matched triple runs the same procedure under
//! L, xz (chords, keeping the y leg) and xyz (straight chords): rebuilt clear under that model, rounds, then rebuilt clear
//! under L and rounds again. So all three end in the same currency, and the L arm gets the second pass too. Prints `ROW`
//! lines; writes plans to `runs/secant/triple/`. No carry or smoothing, and nothing is flown: compare plans, not flights.
//!
//! `--chain MODEL` is one arm from the recording with every round's plan kept, for where the models' lines come apart.
//!
//! `cargo run --release -p academy --bin optimize [--relax | --hybrid] [--plain] [--open-top] [--server] [--margin BLOCKS] [--forbid X0 Y0 Z0 X1 Y1 Z1]... [NAME_SUBSTRING ...]`

use std::path::Path;
use std::process::ExitCode;
use std::sync::OnceLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use etas::course::Course;
use etas::solve::bounces::{SCALE, flight, fly, state_at};
use etas::solve::ddp::{
    Goal, Multipliers, Opts, Plan as DPlan, Problem, Rate, U, X, control, lead_of,
};
use etas::solve::refly::{Repaired, track};
use etas::solve::smooth::{DELTA, WINDOWS as SMOOTH_WINDOWS, smooth_pass};
use etas::solve::stretch::resample;
use etas::solve::walls::Walls;
use etas::trajectory::{Header, InputRow, Trajectory};
use etas::verify::{Plan, verify};
use rayon::prelude::*;
use sim::generic::Generic;
use world::{Aabb, World};

const OBJECTIVE: &str = "finish_gate_segments";
/// Ticks flown past the finish on the last rotation, so a plan outlasts the chat's lag.
const TAIL: usize = 7;
const FIRST: usize = 40;
const WINDOWS: [usize; 6] = [30, 60, 120, 240, 40, 90];
/// Rounds, at most; stops after a round that gains under `MIN_GAIN` ticks.
const ROUNDS: usize = 8;
const MIN_GAIN: f64 = 0.05;
/// [`carry`]'s windows and first cuts, its first try at a window's gain in ticks, and the most it tries.
const CARRY: [(usize, usize); 4] = [(40, 0), (60, 30), (30, 15), (90, 45)];
const CARRY_FIRST: f64 = 0.03;
const CARRY_MOST: f64 = 1.0;
/// How far off a [`carry`] window may end, by [`Goal::Reach`]'s value: blocks and b/t over `SCALE`.
const CARRY_MISS: f64 = 0.05;
/// Infeasible horizons tried below the least that reached, before giving up on a window.
const PATIENCE: usize = 2;
/// A window's shortest horizon, as a fraction of its length.
const MIN_FRACTION: f64 = 0.7;
const MARGIN: f64 = 0.02;
/// The least clearance a plan keeps after stitching and rolling out again, which drifts it
/// by the reaches' tolerance.
const KEEP: f64 = 0.01;
/// Tracker windows, tried until one flies clean; and its margin, under `KEEP`. why under?
/// a reach can't hold a margin the plan it follows doesn't have.
const TRACK_WINDOWS: [usize; 4] = [24, 12, 6, 3];
const TRACK_MARGIN: f64 = 0.004;
/// The clearance a solve is taken at, short of `MARGIN`: the augmented Lagrangian closes on
/// the margin from below, and stops at 0.01999 as often as not.
const LOOSE: f64 = 0.003;

/// The prices [`relax`] steps down, the last 0.
const RELAX: [f64; 3] = [1.0, 0.1, 0.01];
/// The price the finished plan is smoothed at. Free, since the windows keep their end states.
const FINAL: f64 = 0.1;

struct Run {
    name: String,
    rec: Trajectory,
    timer: usize,
    /// The first tick past the finish plane. The finish fires a move later, and the timer
    /// did too, so `gate - (timer - 1)` is the run's timed ticks.
    gate: usize,
}

fn goal(course: &Course, cap: usize) -> Result<Goal, String> {
    let f = &course.region("finish")?.working;
    Ok(Goal::Gate {
        normal: [0.0, 0.0, 1.0],
        at: f.min[2].ok_or("finish has no z_min")?,
        cap,
        lo: [
            f.min[0].unwrap_or(f64::NEG_INFINITY),
            f.min[1].unwrap_or(f64::NEG_INFINITY),
            f64::NEG_INFINITY,
        ],
        hi: f.max.map(|m| m.unwrap_or(f64::INFINITY)),
    })
}

/// `--plain`: `--relax` or `--hybrid` with no price on rotation anywhere, for what a seed is worth on time alone.
static PLAIN: AtomicBool = AtomicBool::new(false);
/// `--server`: hold [`Opts::server`] through the whole optimization.
static SERVER: AtomicBool = AtomicBool::new(false);
/// `--margin BLOCKS`, to see what clearance costs; [`MARGIN`] without it.
static WALL_MARGIN: OnceLock<f64> = OnceLock::new();

/// The collision model every solve runs under, as an index into [`MODELS`]. A process-wide switch, so `--triple` runs its
/// arms one at a time.
static MODEL: AtomicUsize = AtomicUsize::new(0);
/// The L at a wider margin and with shifted windows are `--triple`'s controls: they shake a plan without chords, so what
/// chords gain can be told from what any shake gains. Stored as name, `s`, margin (0 is the run's) and shift in thirds of a window.
const MODELS: [(&str, [f64; 2], f64, usize); 8] = [
    ("L", [0.0, 0.0], 0.0, 0),
    ("xz", [1.0, 0.0], 0.0, 0),
    ("xyz", [1.0, 1.0], 0.0, 0),
    ("Lm.1", [0.0, 0.0], 0.1, 0),
    ("Lm.3", [0.0, 0.0], 0.3, 0),
    ("Lo1", [0.0, 0.0], 0.0, 1),
    ("Lo2", [0.0, 0.0], 0.0, 2),
    // worst of L, xz and xyz at once; see `joint` in `opts`
    ("all", [0.0, 0.0], 0.0, 0),
];

fn model() -> (&'static str, [f64; 2], f64, usize) {
    MODELS[MODEL.load(Ordering::Relaxed)]
}

fn wall_margin() -> f64 {
    match model().2 {
        0.0 => *WALL_MARGIN.get().unwrap_or(&MARGIN),
        m => m,
    }
}

fn opts() -> Opts {
    Opts {
        tol: 1e-6,
        margin: wall_margin(),
        server: SERVER.load(Ordering::Relaxed),
        relax: match model().1 {
            [0.0, 0.0] => None,
            s => Some(Arc::new(vec![s])),
        },
        joint: (model().0 == "all").then(|| Arc::new(vec![MODELS[0].1, MODELS[1].1, MODELS[2].1])),
        ..Opts::default()
    }
}

/// Whether a plan is feasible, its clearance judged [`LOOSE`]ly.
fn solved(pb: &Problem, plan: &DPlan) -> bool {
    let loose = Problem {
        opts: Opts {
            margin: wall_margin() - LOOSE,
            ..pb.opts.clone()
        },
        ..pb.clone()
    };
    loose.feasible(plan)
}

/// The fewest ticks from `x0` to `target`, from `seed`'s length down, or up to `up` more if
/// that fails. None if nothing reaches it.
fn reach(walls: &Walls, x0: X, target: X, seed: &[U], up: usize) -> Option<DPlan> {
    let pb = Problem {
        kernel: Generic::default(),
        walls,
        x0,
        goal: Goal::Reach {
            target,
            scale: SCALE,
        },
        opts: opts(),
    };
    let n0 = seed.len();
    let mut best: Option<DPlan> = None;
    let mut u = seed.to_vec();
    for n in n0..=n0 + up {
        let s = pb.solve(&resample(&u, n));
        if solved(&pb, &s.plan) {
            best = Some(s.plan);
            break;
        }
        u = s.plan.u;
    }
    let mut best = best?;
    let floor = ((n0 as f64 * MIN_FRACTION) as usize).max(2);
    let mut n = best.u.len();
    let mut misses = 0;
    while n > floor && misses <= PATIENCE {
        n -= 1;
        let s = pb.solve(&resample(&best.u, n));
        if solved(&pb, &s.plan) {
            best = s.plan;
            misses = 0;
        } else {
            misses += 1;
        }
    }
    Some(best)
}

/// `u` flown from the problem's start, its horizon fitted to the goal.
fn roll(gate: &Problem, u: &[U]) -> DPlan {
    let mut u = u.to_vec();
    let mut m = Multipliers::new(1.0, u.len());
    let x = gate.refit(&mut m, &mut u);
    DPlan { u, x }
}

/// The plan from the recording: reaches between its states every `FIRST` ticks.
fn first_plan(run: &Run, walls: &Walls, gate: &Problem) -> Result<DPlan, String> {
    let (a, f) = (run.timer - 1, run.gate);
    let ctl = |t: usize| {
        let i = &run.rec.rows[t].input;
        control(lead_of(&flight(state_at(&run.rec, t)), i.yaw), i.pitch)
    };
    let ok = |t: usize| {
        let s = state_at(&run.rec, t);
        s.fall_flying && !s.on_ground && !s.horizontal_collision && !s.vertical_collision
    };
    // Whole, if it will. why? a seed already flown needs no pieces, and pieces lose it a tick: each has to reach the
    // seed's state in the seed's ticks at the solve's margin, which a seed tracked at `TRACK_MARGIN` doesn't have, so one
    // that can't takes a tick more and the rounds never get it back (academy, 2026-11-06: a 527.99 came back 528.94).
    if (a..f).all(ok) {
        let seed: Vec<U> = (a..f).map(ctl).collect();
        let s = gate.solve(&seed);
        if solved(gate, &s.plan) {
            return Ok(s.plan);
        }
    }
    let mut cuts = vec![a];
    let mut t = a + FIRST;
    while t + FIRST / 2 < f {
        // a cut on a tick in normal flight; a contact's neighbors won't do
        let c = (t..f)
            .find(|&c| ok(c) && ok(c + 1))
            .ok_or("no flight to cut at")?;
        cuts.push(c);
        t = c + FIRST;
    }
    cuts.push(f);
    pieces_plan(&cuts, ctl, |t| flight(state_at(&run.rec, t)), walls, gate)
}

/// A plan through `state` at each of `cuts`, by reaches seeded from `ctl`, the last piece to the gate.
fn pieces_plan(
    cuts: &[usize],
    ctl: impl Fn(usize) -> U + Sync,
    state: impl Fn(usize) -> X + Sync,
    walls: &Walls,
    gate: &Problem,
) -> Result<DPlan, String> {
    let f = *cuts.last().ok_or("no cuts")?;
    let piece = |i: usize, j: usize| -> Result<Vec<U>, String> {
        let (from, to) = (cuts[i], cuts[j]);
        let seed: Vec<U> = (from..to).map(&ctl).collect();
        let x0 = state(from);
        if to == f {
            let pb = Problem { x0, ..gate.clone() };
            let s = pb.solve(&seed);
            if solved(&pb, &s.plan) {
                return Ok(s.plan.u);
            }
            return Err(format!(
                "last piece {from}..{f}: value {:.2}, clearance {:.3}",
                pb.value(&s.plan.x),
                pb.worst(&s.plan.x, &s.plan.u)
            ));
        }
        reach(walls, x0, state(to), &seed, 12)
            .map(|p| p.u)
            .ok_or(format!("piece {from}..{to}: no reach"))
    };
    let pieces: Vec<Result<Vec<U>, String>> = (0..cuts.len() - 1)
        .into_par_iter()
        .map(|i| piece(i, i + 1))
        .collect();
    // a piece that fails ends on a state only contact gets to (a wall hit's): it merges
    // with the next, or the last with the one before
    let mut u = Vec::new();
    let mut i = 0;
    let last = cuts.len() - 1;
    while i < last {
        let mut j = i + 1;
        let mut got = pieces[i].clone();
        while got.is_err() && j < last {
            j += 1;
            got = piece(i, j);
        }
        if let Err(e) = got.clone() {
            // the tail of the run: give back what the pieces before it solved
            let mut k = i;
            while k > 0 && got.is_err() {
                k -= 1;
                let solved_before: usize = pieces[k].as_ref().map_or(0, |p| p.len());
                u.truncate(u.len().saturating_sub(solved_before));
                got = piece(k, last);
            }
            if got.is_err() {
                return Err(e);
            }
        }
        u.extend(got?);
        i = j;
    }
    Ok(roll(gate, &u))
}

/// One pass of windows of `w` ticks, the first cut at `off`. A window that doesn't solve
/// keeps the plan's controls.
fn pass(walls: &Walls, gate: &Problem, plan: &DPlan, w: usize, off: usize) -> Vec<U> {
    let n = plan.u.len();
    let mut cuts = vec![0];
    let mut t = if off == 0 { w } else { off };
    while t + w / 2 < n {
        cuts.push(t);
        t += w;
    }
    cuts.push(n);
    let pieces: Vec<Vec<U>> = cuts
        .par_windows(2)
        .map(|c| {
            let seed = &plan.u[c[0]..c[1]];
            if c[1] == n {
                let pb = Problem {
                    x0: plan.x[c[0]],
                    ..gate.clone()
                };
                let s = pb.solve(seed);
                if solved(&pb, &s.plan) {
                    return s.plan.u;
                }
                return seed.to_vec();
            }
            match reach(walls, plan.x[c[0]], plan.x[c[1]], seed, 0) {
                Some(p) if p.u.len() <= seed.len() => p.u,
                _ => seed.to_vec(),
            }
        })
        .collect();
    pieces.concat()
}

/// Windows of `w` ticks from `off`, in order, each flown to the plan's state further along than its ticks: to
/// where the plan is at `at + w + gain`, for the most gain it solves. A plan's state between ticks is the line
/// between them. What's gained is carried into the next window, so the plan's fractions add up, and the last
/// window is the gate's. If a window can't hold what's carried, the gate takes over from there.
fn carry(walls: &Walls, gate: &Problem, plan: &DPlan, w: usize, off: usize, log: &mut Vec<String>) -> Vec<U> {
    let n = plan.u.len();
    let state = |t: f64| -> X {
        let i = (t.floor() as usize).min(n - 1);
        let f = t - i as f64;
        std::array::from_fn(|k| plan.x[i][k] * (1.0 - f) + plan.x[i + 1][k] * f)
    };
    let (mut at, mut x, mut u) = (0.0f64, plan.x[0], Vec::new());
    let mut gains = Vec::new();
    let mut next = if off == 0 { w } else { off };
    loop {
        let ticks = next;
        next = w;
        let ahead = (at + ticks as f64 + 1.0).ceil() as usize;
        if ahead + w / 2 >= n {
            break;
        }
        let from = at.round() as usize;
        let seed = &plan.u[from..(from + ticks).min(n)];
        let aim = |gain: f64| {
            let p = Problem {
                kernel: Generic::default(),
                walls,
                x0: x,
                goal: Goal::Reach {
                    target: state(at + ticks as f64 + gain),
                    scale: SCALE,
                },
                opts: opts(),
            };
            let s = p.solve(&resample(seed, ticks));
            // why loose? the next window starts where this one ended, so a near miss costs nothing
            let judge = Problem {
                opts: Opts { tol: CARRY_MISS, ..p.opts.clone() },
                ..p.clone()
            };
            solved(&judge, &s.plan).then_some(s.plan)
        };
        let mut got = None;
        let mut gain = CARRY_FIRST;
        while gain <= CARRY_MOST {
            let Some(p) = aim(gain) else { break };
            got = Some((gain, p));
            gain *= 2.0;
        }
        if got.is_none() {
            got = aim(CARRY_FIRST / 3.0).map(|p| (CARRY_FIRST / 3.0, p));
        }
        if got.is_none() {
            got = aim(0.0).map(|p| (0.0, p));
        }
        let Some((gain, p)) = got else { break };
        gains.push(gain);
        at += ticks as f64 + gain;
        x = *p.x.last().unwrap();
        u.extend(p.u);
    }
    let from = (at.round() as usize).min(n - 1);
    let pb = Problem {
        x0: x,
        opts: Opts {
            c0: 1e3,
            iters: 300,
            ..opts()
        },
        ..gate.clone()
    };
    let s = pb.solve(&plan.u[from..]);
    let ok = solved(&pb, &s.plan);
    log.push(format!("\n  carry {w}: {:.2} over {} windows", at - u.len() as f64, gains.len()));
    u.extend(if ok { s.plan.u } else { plan.u[from..].to_vec() });
    u
}

/// Keep `new` over `best` if it is clear of the walls, in the gate, and faster.
fn better(gate: &Problem, new: DPlan, best: &mut DPlan) -> bool {
    let n = new.x.len() - 1;
    let ok = gate.worst(&new.x, &new.u) >= KEEP
        && n > 0
        && gate.crossed(&new.x[n - 1], &new.x[n])
        && gate.value(&new.x) < gate.value(&best.x);
    if ok {
        *best = new;
    }
    ok
}

/// Passes of [`smooth_pass`] at `weight`, each kept if it's clear and no slower.
fn smooth(walls: &Walls, gate: &Problem, plan: &mut DPlan, weight: f64, log: &mut Vec<String>) {
    let before = gate.travel(&plan.x, &plan.u);
    let mut kept = 0;
    let priced = Opts {
        rate: Rate::new(weight, DELTA),
        ..opts()
    };
    for (w, off) in SMOOTH_WINDOWS {
        let (piecewise, k) = smooth_pass(walls, plan, w, off, &priced, LOOSE);
        let new = roll(gate, &piecewise.u);
        let n = new.x.len() - 1;
        if gate.worst(&new.x, &new.u) >= KEEP
            && n > 0
            && gate.crossed(&new.x[n - 1], &new.x[n])
            && gate.value(&new.x) <= gate.value(&plan.x) + 0.01
        {
            *plan = new;
            kept += k;
        }
    }
    let t = gate.travel(&plan.x, &plan.u);
    log.push(format!(
        "\n  smooth {weight}: {:.2}, {kept} pieces, yaw {:.0}>{:.0} pitch {:.0}>{:.0}, small {:.0}>{:.0} {:.0}>{:.0}",
        gate.value(&plan.x),
        before[0], t[0], before[1], t[1], before[2], t[2], before[3], t[3],
    ));
}

/// [`ROUNDS`] of window passes and a whole solve, stopping at a round short of [`MIN_GAIN`]. `each` sees every round's plan.
fn rounds(walls: &Walls, gate: &Problem, best: &mut DPlan, log: &mut Vec<String>, mut each: impl FnMut(usize, &DPlan)) {
    let stiff = Problem {
        opts: Opts {
            c0: 1e3,
            iters: 300,
            ..opts()
        },
        ..gate.clone()
    };
    let mut i = 0;
    for round in 0..ROUNDS {
        let before = gate.value(&best.x);
        for w in WINDOWS {
            i += 1;
            let off = (round % 2) * w / 2 + (i % 3) * w / 7 + model().3 * w / 3;
            let new = roll(gate, &pass(walls, gate, best, w, off));
            better(gate, new, best);
        }
        let windows = gate.value(&best.x);
        let s = stiff.solve(&best.u);
        better(gate, s.plan, best);
        log.push(format!("{windows:.2}>{:.2}", gate.value(&best.x)));
        each(round, best);
        if before - gate.value(&best.x) < MIN_GAIN {
            break;
        }
    }
}

/// `plan` made clear under the current model: as it is if it already is, else reaches through its own states.
fn rebuilt(plan: &DPlan, walls: &Walls, gate: &Problem) -> Result<DPlan, String> {
    if solved(gate, plan) {
        return Ok(plan.clone());
    }
    let n = plan.u.len();
    // a cut must end on a tick clear under this model: a reach can't move the tick that arrives at its target
    let clear = |k: usize| gate.worst(&plan.x[k - 1..=k], &plan.u[k - 1..k]) >= wall_margin();
    let mut cuts = vec![0];
    let mut t = FIRST;
    while t + FIRST / 2 < n {
        match (t..n).find(|&c| clear(c)) {
            Some(c) if c + FIRST / 2 < n => {
                cuts.push(c);
                t = c + FIRST;
            }
            _ => break,
        }
    }
    cuts.push(n);
    pieces_plan(&cuts, |t| plan.u[t], |t| plan.x[t], walls, gate)
}

/// One solve's reading, on stdout and to `runs/secant/triple/`.
fn row(dir: &Path, walls: &Walls, gate: &Problem, tag: &str, phase: &str, p: &DPlan, secs: f64) -> Result<(), String> {
    let covered = p.x.iter().all(|x| walls.covers([x[0], x[1], x[2]]));
    let own = solved(gate, p) && covered;
    let m = MODEL.swap(0, Ordering::Relaxed);
    let l = Problem { opts: opts(), ..gate.clone() };
    let (lw, lok) = (l.worst(&p.x, &p.u), solved(&l, p) && covered);
    MODEL.store(m, Ordering::Relaxed);
    println!(
        "ROW\t{tag}\t{}\t{phase}\t{:.4}\t{:.4}\t{own}\t{lw:.4}\t{lok}\t{}\t{secs:.1}",
        MODELS[m].0,
        gate.value(&p.x),
        gate.worst(&p.x, &p.u),
        p.u.len(),
    );
    dump(dir, tag, &format!("{}_{phase}", MODELS[m].0), p)
}

fn dump(dir: &Path, tag: &str, what: &str, p: &DPlan) -> Result<(), String> {
    let out = dir.join("runs/secant/triple");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let rows: Vec<String> = p.u.iter().zip(&p.x).map(|(u, x)| format!("{}\t{}\t{}\t{}\t{}", u[0], u[1], x[0], x[1], x[2])).collect();
    std::fs::write(out.join(format!("{}_{what}.tsv", tag.replace('\t', "_"))), rows.join("\n")).map_err(|e| e.to_string())
}

/// One arm of a triple: `start` (None: the recording) under the current model, then under L.
fn arm(dir: &Path, course: &Course, world: &World, walls: &Walls, run: &Run, tag: &str, start: Option<&DPlan>, mut each: impl FnMut(usize, &DPlan)) -> Result<(), String> {
    let a = run.timer - 1;
    let problem = || -> Result<Problem, String> {
        Ok(Problem {
            kernel: Generic::default(),
            walls,
            x0: flight(state_at(&run.rec, a)),
            goal: goal(course, run.gate - a + 80)?,
            opts: opts(),
        })
    };
    let gate = problem()?;
    let t0 = Instant::now();
    let mut best = match start {
        Some(p) => rebuilt(p, walls, &gate)?,
        None => first_plan(run, walls, &gate)?,
    };
    row(dir, walls, &gate, tag, "rebuilt", &best, t0.elapsed().as_secs_f64())?;
    let t0 = Instant::now();
    let mut log = Vec::new();
    rounds(walls, &gate, &mut best, &mut log, &mut each);
    row(dir, walls, &gate, tag, "explored", &best, t0.elapsed().as_secs_f64())?;
    // the same again under L, for every arm
    let m = MODEL.swap(0, Ordering::Relaxed);
    let l = problem()?;
    let t0 = Instant::now();
    let back = rebuilt(&best, walls, &l);
    let done = back.and_then(|mut b| {
        row(dir, walls, &l, tag, "back_rebuilt", &b, t0.elapsed().as_secs_f64())?;
        let t0 = Instant::now();
        let mname = MODELS[m].0;
        rounds(walls, &l, &mut b, &mut log, |r, p| {
            let _ = dump(dir, tag, &format!("{mname}_backround{r}"), p);
        });
        row(dir, walls, &l, tag, "back", &b, t0.elapsed().as_secs_f64())?;
        let (rep, traj, _) = fly_plan(course, world, &l, &b, run)?;
        let r = verify(course, world, Plan::Trajectory(&traj));
        println!(
            "FLY\t{tag}\t{}\t{}\t{}\tcontacts {}\tsecant {}",
            MODELS[m].0,
            r.timer_tick().zip(r.finish_tick()).map_or("-".into(), |(s, e)| (e - s).to_string()),
            if r.ok() { "PASS" } else { "FAIL" },
            rep.contacts.len(),
            r.secant_seconds().map_or("-".into(), |s| format!("{:.2}", s / 0.05)),
        );
        traj.write(&dir.join(format!("runs/secant/triple/{}_{}.traj.jsonl", tag.replace('\t', "_"), MODELS[m].0)))
    });
    MODEL.store(m, Ordering::Relaxed);
    done
}

fn triple(dir: &Path, course: &Course, world: &World, walls: &Walls, run: &Run) {
    use std::io::Write;
    println!("HEAD\trun\tcheckpoint\tmodel\tphase\tvalue\tclear\tvalid\tl_clear\tl_valid\tticks\tsecs");
    let branch = |tag: &str, start: Option<&DPlan>, first: usize| {
        for m in first..MODELS.len() {
            MODEL.store(m, Ordering::Relaxed);
            if let Err(e) = arm(dir, course, world, walls, run, tag, start, |_, _| {}) {
                println!("FAIL\t{tag}\t{}\t{e}", MODELS[m].0);
            }
            std::io::stdout().flush().ok();
        }
        MODEL.store(0, Ordering::Relaxed);
    };
    // the chain: the recording's L arm, kept for its checkpoints
    let mut marks: Vec<(String, DPlan)> = Vec::new();
    let tag = format!("{}\trec", run.name);
    let mut last = None;
    if let Err(e) = arm(dir, course, world, walls, run, &tag, None, |r, p| {
        if r == 0 || r == 2 {
            marks.push((format!("r{}", r + 1), p.clone()));
        }
        last = Some((format!("r{}", r + 1), p.clone()));
    }) {
        println!("FAIL\t{tag}\tL\t{e}");
    }
    if let Some((name, p)) = last
        && !marks.iter().any(|(n, _)| *n == name)
    {
        marks.push((format!("end_{name}"), p));
    }
    branch(&tag, None, 1);
    for (name, p) in &marks {
        branch(&format!("{}\t{name}", run.name), Some(p), 0);
    }
}

fn one(
    relaxing: bool,
    dir: &Path,
    course: &Course,
    world: &World,
    walls: &Walls,
    run: &Run,
) -> Result<String, String> {
    let t0 = Instant::now();
    let a = run.timer - 1;
    let x0 = flight(state_at(&run.rec, a));
    let gate = Problem {
        kernel: Generic::default(),
        walls,
        x0,
        goal: goal(course, run.gate - a + 80)?,
        opts: opts(),
    };
    let mut best = first_plan(run, walls, &gate)?;
    let mut log = vec![format!(
        "first {:.2} clear {:.3}",
        gate.value(&best.x),
        gate.worst(&best.x, &best.u)
    )];
    if relaxing && !PLAIN.load(Ordering::Relaxed) {
        for weight in RELAX {
            smooth(walls, &gate, &mut best, weight, &mut log);
        }
    }
    rounds(walls, &gate, &mut best, &mut log, |_, _| {});
    for (w, off) in CARRY {
        let new = roll(&gate, &carry(walls, &gate, &best, w, off, &mut log));
        better(&gate, new, &mut best);
        log.push(format!("to {:.2}", gate.value(&best.x)));
    }
    if relaxing && !PLAIN.load(Ordering::Relaxed) {
        smooth(walls, &gate, &mut best, FINAL, &mut log);
    }
    let generic = gate.value(&best.x);
    let t1 = t0.elapsed().as_secs_f64();

    let (rep, traj, stopped) = fly_plan(course, world, &gate, &best, run)?;
    let out = dir.join(if relaxing {
        "runs/relax"
    } else {
        "runs/optimize"
    });
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    traj.write(&out.join(format!("{}.traj.jsonl", run.name)))?;
    let r = verify(course, world, Plan::Trajectory(&traj));
    let timed = r.timer_tick().zip(r.finish_tick()).map(|(s, e)| e - s);
    Ok(format!(
        "{name}: recorded {rec} -> {res} {verdict}{why}\n  generic {generic:.2} in {t1:.0} s [{log}]\n  track: {nt} ticks (plan {np}), {rs} re-solves, {rt:.1} s, contacts {c:?}, event {e:?}, port stopped {stopped:?}, secant {sec}",
        name = run.name,
        rec = run.gate + 1 - run.timer,
        res = timed.map_or("-".into(), |t| format!("{t} ({:.2} s)", t as f64 * 0.05)),
        verdict = if r.ok() { "PASS" } else { "FAIL" },
        why = r
            .failures
            .first()
            .map_or(String::new(), |e| format!(": {e}")),
        log = log.join(" "),
        nt = rep.inputs.len(),
        np = best.u.len(),
        rs = rep.resolves,
        rt = rep.secs,
        c = &rep.contacts[..rep.contacts.len().min(8)],
        e = rep.event,
        sec = r
            .secant_seconds()
            .map_or("-".into(), |s| format!("{:.2}", s / 0.05)),
    ))
}

/// A plan tracked under the faithful kernel and flown with port from the recording's prefix.
fn fly_plan(course: &Course, world: &World, gate: &Problem, best: &DPlan, run: &Run) -> Result<(Repaired, Trajectory, Option<(usize, port::Unsupported)>), String> {
    let a = run.timer - 1;
    let yaw_from = state_at(&run.rec, a).y_rot;
    let tracker = Problem {
        opts: Opts {
            margin: TRACK_MARGIN,
            ..opts()
        },
        ..gate.clone()
    };
    let mut reps: Vec<_> = TRACK_WINDOWS
        .iter()
        .map(|&w| track(world, &tracker, &best, yaw_from, w, SCALE))
        .collect();
    let at = reps.iter().position(|r| r.clean()).unwrap_or(0);
    let rep = reps.swap_remove(at);

    let mut inputs: Vec<InputRow> = run.rec.rows[..a].iter().map(|r| r.input.clone()).collect();
    let keys = run.rec.rows[a].input.keys.replace('J', "");
    let row = |u: &sim::state::Input<f32>| InputRow {
        yaw: u.yaw as f64,
        pitch: u.pitch as f64,
        keys: keys.clone(),
    };
    inputs.extend(rep.inputs.iter().map(row));
    let last = *rep.inputs.last().ok_or("empty track")?;
    inputs.extend((0..TAIL).map(|_| row(&last)));
    let mut h = Header::new(
        Some(course),
        OBJECTIVE,
        &format!("optimize: {} from tick {a}", run.name),
    );
    h.tas_header = run.rec.header.tas_header.clone();
    let arrivals: Vec<_> = run
        .rec
        .arrivals
        .iter()
        .filter(|x| x.tick <= a)
        .copied()
        .collect();
    let (traj, stopped) = fly(&run.rec.initial, &inputs, &arrivals, world, h);
    Ok((rep, traj, stopped))
}

/// The timer start and the gate, by the rows' positions alone: the move that starts the timer,
/// and the first crossing of the finish plane after it.
fn scan_ticks(course: &Course, rec: &Trajectory) -> Result<Option<(usize, usize)>, String> {
    let start = &course.region("start_area")?.working;
    let at = course.region("finish")?.working.min[2].ok_or("finish has no z_min")?;
    let pos = |t: usize| state_at(rec, t).position;
    let n = rec.rows.len();
    let timer = start.exit_move((0..n).map(pos));
    let gate = timer.and_then(|a| (a..n).find(|&t| pos(t - 1)[2] <= at && pos(t)[2] > at));
    Ok(timer.zip(gate))
}

fn run() -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (mut course, world) = Course::load(dir)?;
    let mut only: Vec<String> = std::env::args().skip(1).collect();
    let hybrids = only.iter().any(|a| a == "--hybrid");
    SERVER.store(only.iter().any(|a| a == "--server"), Ordering::Relaxed);
    PLAIN.store(only.iter().any(|a| a == "--plain"), Ordering::Relaxed);
    if let Some(k) = only.iter().position(|a| a == "--margin") {
        let m: f64 = only.get(k + 1).and_then(|a| a.parse().ok()).expect("--margin BLOCKS");
        WALL_MARGIN.set(m).unwrap();
        only.drain(k..=k + 1);
    }
    let mut forbid = Vec::new();
    while let Some(k) = only.iter().position(|a| a == "--forbid") {
        let v: Vec<f64> = only[k + 1..k + 7].iter().map(|a| a.parse().expect("--forbid X0 Y0 Z0 X1 Y1 Z1")).collect();
        forbid.push(Aabb { min: [v[0], v[1], v[2]], max: [v[3], v[4], v[5]] });
        only.drain(k..k + 7);
    }
    let relaxing = hybrids || only.iter().any(|a| a == "--relax");
    let triples = only.iter().any(|a| a == "--triple");
    let chain = only.iter().position(|a| a == "--chain").map(|k| {
        let m = only.remove(k + 1);
        only.remove(k);
        MODELS.iter().position(|x| x.0 == m).expect("--chain MODEL")
    });
    if only.iter().any(|a| a == "--open-top") {
        let start = course.regions.get_mut("start_area").ok_or("no start area")?;
        start.working.max[1] = start.outer.max[1];
    }
    only.retain(|a| a != "--relax" && a != "--hybrid" && a != "--open-top" && a != "--plain" && a != "--triple");
    let mut runs = Vec::new();
    let seeds = if hybrids {
        "runs/hybrid"
    } else if relaxing {
        "runs/optimize"
    } else {
        "runs/flights"
    };
    let mut paths: Vec<_> = std::fs::read_dir(dir.join(seeds))
        .map_err(|e| format!("{seeds}: {e}; run the phase before it first"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".traj.jsonl"))
        .collect();
    paths.sort();
    for p in paths {
        let name = p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches(".traj.jsonl")
            .to_string();
        if !only.is_empty() && !only.iter().any(|o| name.contains(o.as_str())) {
            continue;
        }
        let rec = Trajectory::read(&p)?;
        let r = verify(&course, &world, Plan::Trajectory(&rec));
        // a hybrid's rows don't replay, so verify stops at the splice: only the positions count
        let ticks = if hybrids || r.timer_tick().zip(r.finish_tick()).is_some() {
            scan_ticks(&course, &rec)?
        } else {
            None
        };
        match ticks {
            Some((timer, gate)) => runs.push(Run {
                name,
                rec,
                timer,
                gate,
            }),
            None => println!("{name}: no timer or finish in the seed: {:?}", r.failures),
        }
    }
    let pts: Vec<[f64; 3]> = runs
        .iter()
        .flat_map(|r| r.rec.rows.iter().map(|x| x.state.position))
        .collect();
    let walls = Walls::around(&world, &pts, 6.0, 1.0).with_boxes(&forbid);
    if let Some(m) = chain {
        for r in &runs {
            MODEL.store(m, Ordering::Relaxed);
            let tag = format!("{}\tchain", r.name);
            let keep = |i: usize, p: &DPlan| {
                let _ = dump(dir, &tag, &format!("{}_round{i}", MODELS[m].0), p);
            };
            if let Err(e) = arm(dir, &course, &world, &walls, r, &tag, None, keep) {
                println!("FAIL\t{tag}\t{e}");
            }
        }
        return Ok(());
    }
    if triples {
        for r in &runs {
            triple(dir, &course, &world, &walls, r);
        }
        return Ok(());
    }
    runs.par_iter()
        .for_each(|r| match one(relaxing, dir, &course, &world, &walls, r) {
            Ok(s) => println!("{s}"),
            Err(e) => println!("{}: {e}", r.name),
        });
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("optimize: {e}");
            ExitCode::FAILURE
        }
    }
}
