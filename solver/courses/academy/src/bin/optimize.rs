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
//! Then `refly::track` flies the plan under the faithful kernel.
//!
//! With `--relax`, the seeds are `runs/optimize/`'s flights and the results go to
//! `runs/relax/`: the first plan, [`smooth`]ed at each price in `RELAX`, the rounds again, and
//! [`smooth`] once more at `FINAL`.
//!
//! With `--hybrid`, the same on `runs/hybrid/`: a flight with a stretch from another attempt, to
//! try its route.
//!
//! `cargo run --release -p academy --bin optimize [--relax | --hybrid] [NAME_SUBSTRING ...]`

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use etas::course::Course;
use etas::solve::bounces::{SCALE, flight, fly, state_at};
use etas::solve::ddp::{
    Goal, Multipliers, Opts, Plan as DPlan, Problem, Rate, U, X, control, lead_of, rotation,
};
use etas::solve::refly::track;
use etas::solve::stretch::resample;
use etas::solve::walls::Walls;
use etas::trajectory::{Header, InputRow, Trajectory};
use etas::verify::{Plan, verify};
use rayon::prelude::*;
use sim::generic::Generic;
use world::World;

const OBJECTIVE: &str = "finish_gate_segments";
/// Ticks flown past the finish on the last rotation, so a plan outlasts the chat's lag.
const TAIL: usize = 7;
const FIRST: usize = 40;
const WINDOWS: [usize; 6] = [30, 60, 120, 240, 40, 90];
/// Rounds, at most; stops after a round that gains under `MIN_GAIN` ticks.
const ROUNDS: usize = 8;
const MIN_GAIN: f64 = 0.05;
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

/// The prices [`relax`] steps down, the last 0. why this `DELTA`, radians (5 degrees)? chatter
/// is smaller than that, and a route's snaps are far bigger.
const RELAX: [f64; 3] = [1.0, 0.1, 0.01];
const DELTA: f64 = 0.087;
/// The price the finished plan is smoothed at. Free, since the windows keep their end states.
const FINAL: f64 = 0.1;
/// [`smooth_pass`]'s window lengths and first cuts.
const SMOOTH_WINDOWS: [(usize, usize); 6] =
    [(40, 0), (40, 20), (25, 12), (60, 30), (25, 0), (40, 10)];

struct Run {
    name: String,
    rec: Trajectory,
    timer: usize,
    finish: usize,
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

fn opts() -> Opts {
    Opts {
        tol: 1e-6,
        margin: MARGIN,
        ..Opts::default()
    }
}

/// Whether a plan is feasible, its clearance judged [`LOOSE`]ly.
fn solved(pb: &Problem, plan: &DPlan) -> bool {
    let loose = Problem {
        opts: Opts {
            margin: MARGIN - LOOSE,
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
    let (a, f) = (run.timer - 1, run.finish);
    let ctl = |t: usize| {
        let i = &run.rec.rows[t].input;
        control(lead_of(&flight(state_at(&run.rec, t)), i.yaw), i.pitch)
    };
    let ok = |t: usize| {
        let s = state_at(&run.rec, t);
        s.fall_flying && !s.on_ground && !s.horizontal_collision && !s.vertical_collision
    };
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
    let piece = |i: usize, j: usize| -> Result<Vec<U>, String> {
        let (from, to) = (cuts[i], cuts[j]);
        let seed: Vec<U> = (from..to).map(ctl).collect();
        let x0 = flight(state_at(&run.rec, from));
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
        reach(walls, x0, flight(state_at(&run.rec, to)), &seed, 12)
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

/// One pass of [`Rate`]-priced windows of `w` ticks: each piece flown between the plan's own
/// states in the same ticks, so the time doesn't change. A piece is kept if it solves at a
/// lower price. The piece to the finish stays. Returns the controls and the pieces kept.
fn smooth_pass(walls: &Walls, plan: &DPlan, w: usize, off: usize, weight: f64) -> (Vec<U>, usize) {
    let n = plan.u.len();
    let mut cuts = vec![0];
    let mut t = if off == 0 { w } else { off };
    while t + w / 2 < n {
        cuts.push(t);
        t += w;
    }
    let pieces: Vec<(Vec<U>, bool)> = cuts
        .par_windows(2)
        .map(|c| {
            let seed = &plan.u[c[0]..c[1]];
            let from = (c[0] > 0).then(|| rotation(&plan.x[c[0] - 1], &plan.u[c[0] - 1]));
            let pb = Problem {
                kernel: Generic::default(),
                walls,
                x0: plan.x[c[0]],
                goal: Goal::Reach {
                    target: plan.x[c[1]],
                    scale: SCALE,
                },
                opts: Opts {
                    rate: Rate {
                        from,
                        ..Rate::new(weight, DELTA)
                    },
                    ..opts()
                },
            };
            let s = pb.solve(seed);
            let lower =
                pb.rate_price(&s.plan.x, &s.plan.u) < pb.rate_price(&plan.x[c[0]..=c[1]], seed);
            if solved(&pb, &s.plan) && lower {
                (s.plan.u, true)
            } else {
                (seed.to_vec(), false)
            }
        })
        .collect();
    let kept = pieces.iter().filter(|p| p.1).count();
    let mut u: Vec<U> = pieces.into_iter().flat_map(|p| p.0).collect();
    u.extend_from_slice(&plan.u[*cuts.last().unwrap()..]);
    (u, kept)
}

/// Passes of [`smooth_pass`] at `weight`, each kept if it's clear and no slower.
fn smooth(walls: &Walls, gate: &Problem, plan: &mut DPlan, weight: f64, log: &mut Vec<String>) {
    let before = gate.travel(&plan.x, &plan.u);
    let mut kept = 0;
    for (w, off) in SMOOTH_WINDOWS {
        let (u, k) = smooth_pass(walls, plan, w, off, weight);
        let new = roll(gate, &u);
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
        goal: goal(course, run.finish - a + 80)?,
        opts: opts(),
    };
    let mut best = first_plan(run, walls, &gate)?;
    let mut log = vec![format!(
        "first {:.2} clear {:.3}",
        gate.value(&best.x),
        gate.worst(&best.x, &best.u)
    )];
    if relaxing {
        for weight in RELAX {
            smooth(walls, &gate, &mut best, weight, &mut log);
        }
    }
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
            let off = (round % 2) * w / 2 + (i % 3) * w / 7;
            let new = roll(&gate, &pass(walls, &gate, &best, w, off));
            better(&gate, new, &mut best);
        }
        let windows = gate.value(&best.x);
        let s = stiff.solve(&best.u);
        better(&gate, s.plan, &mut best);
        log.push(format!("{windows:.2}>{:.2}", gate.value(&best.x)));
        if before - gate.value(&best.x) < MIN_GAIN {
            break;
        }
    }
    if relaxing {
        smooth(walls, &gate, &mut best, FINAL, &mut log);
    }
    let generic = gate.value(&best.x);
    let t1 = t0.elapsed().as_secs_f64();

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
        rec = run.finish + 1 - run.timer,
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

/// The timer start and the finish, by the rows' positions alone: the first tick past the start
/// area, and the first crossing into the finish.
fn scan_ticks(course: &Course, rec: &Trajectory) -> Result<Option<(usize, usize)>, String> {
    let start = &course.region("start_area")?.working;
    let at = course.region("finish")?.working.min[2].ok_or("finish has no z_min")?;
    let pos = |t: usize| state_at(rec, t).position;
    let n = rec.rows.len();
    let timer = (1..n).find(|&t| !start.contains(pos(t)));
    let finish = timer.and_then(|a| (a..n).find(|&t| pos(t - 1)[2] <= at && pos(t)[2] > at));
    Ok(timer.zip(finish))
}

fn run() -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (course, world) = Course::load(dir)?;
    let mut only: Vec<String> = std::env::args().skip(1).collect();
    let hybrids = only.iter().any(|a| a == "--hybrid");
    let relaxing = hybrids || only.iter().any(|a| a == "--relax");
    only.retain(|a| a != "--relax" && a != "--hybrid");
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
        // a hybrid's rows don't replay, so verify stops at the splice: its ticks by position
        let ticks = if hybrids {
            scan_ticks(&course, &rec)?
        } else {
            r.timer_tick().zip(r.finish_tick())
        };
        match ticks {
            Some((timer, finish)) => runs.push(Run {
                name,
                rec,
                timer,
                finish,
            }),
            None => println!("{name}: no timer or finish in the seed: {:?}", r.failures),
        }
    }
    let pts: Vec<[f64; 3]> = runs
        .iter()
        .flat_map(|r| r.rec.rows.iter().map(|x| x.state.position))
        .collect();
    let walls = Walls::around(&world, &pts, 6.0, 1.0);
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
