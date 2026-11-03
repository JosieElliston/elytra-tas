//! Phase `resolve`: re-solve stretches of the recorded run and splice them back.
//!
//! Per stretch `a..b` (the recorded states at the ends of ticks `a` and `b`):
//!
//! 1. Objective `reach_recorded_state_min_ticks`: from the recorded state at `a`,
//!    reach the recorded state at `b` (position and velocity) in as few ticks as
//!    possible. DDP through ghost walls on the generic kernel solves each horizon
//!    `N = b - a, b - a - 1, ...` until one can't reach it; the last that can wins.
//!    why this goal? the recorded suffix then replays from the same state, so the
//!    stitched run keeps the rest of the recording; a crossing goal frees the state
//!    at the end and the replayed suffix would fly from somewhere else.
//! 2. Faithful: the plan's rotations open loop (where it parts), then repaired by
//!    re-solving from the faithful state every tick (`solve::refly::repair`).
//! 3. Splice: recorded prefix, the repaired stretch, the recorded suffix, flown with
//!    port, written to `runs/resolve_A_B.traj.jsonl`, and verified.
//! 4. Diagnostic, objective `plane_secant`: the least secant time to the plane through
//!    the recorded position at `b` across its velocity, with the state there free.
//!    It shows what a crossing goal would claim and what it spends to get it (height,
//!    speed); it isn't spliced.
//!
//! `cargo run --release -p burrowed_grove --bin resolve [A..B ...]`

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use etas::course::Course;
use etas::solve::ddp::{Goal, Opts, Problem, Solved, U, X};
use etas::solve::refly::{refly, repair};
use etas::solve::stretch::{Stretch, resample, splice, stretch};
use etas::solve::walls::Walls;
use etas::trajectory::{Header, Trajectory};
use etas::verify::{Plan as VPlan, verify};
use sim::generic::Generic;
use sim::state::Input;
use world::World;

const OBJECTIVE: &str = "reach_recorded_state_min_ticks";
const DEFAULT: [(usize, usize); 6] = [
    (90, 150),
    (180, 220),
    (230, 310),
    (400, 480),
    (520, 580),
    (600, 640),
];
/// Errors over this scale count toward `tol`: position in blocks, velocity in blocks
/// per 20 ticks.
const SCALE: X = [1.0, 1.0, 1.0, 0.05, 0.05, 0.05];
/// The search tries horizons down to this fraction of the recorded one.
const MIN_FRACTION: f64 = 0.6;

fn walls_for(world: &World, pts: &[[f64; 3]]) -> Walls {
    Walls::around(world, pts, 4.0, 1.0)
}

/// Solve `pb` from `seed`; if the plan leaves the walls' region, grow it around the
/// plan and solve again.
fn solve_covered(
    world: &World,
    walls: &mut Walls,
    path: &[[f64; 3]],
    make: impl Fn(&Walls) -> Problem<'_>,
    seed: &[U],
) -> Solved {
    let mut pts = path.to_vec();
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
        *walls = walls_for(world, &pts);
    }
}

fn opts() -> Opts {
    Opts {
        tol: 1e-6,
        ..Opts::default()
    }
}

struct Outcome {
    line: String,
    ok: bool,
    /// The repaired rotations, when the stitched run verified.
    inputs: Option<Vec<Input<f32>>>,
}

fn header(course: &Course, rec: &Trajectory, objective: &str, source: String) -> Header {
    let mut h = Header::new(Some(course), objective, &source);
    h.tas_header = rec.header.tas_header.clone();
    h
}

fn one(
    dir: &Path,
    course: &Course,
    world: &World,
    rec: &Trajectory,
    base: (usize, f64),
    st: &Stretch,
) -> Result<Outcome, String> {
    let kernel = Generic::default();
    let n = st.b - st.a;
    let mut walls = walls_for(world, &st.path);
    let reach = Goal::Reach {
        target: st.target,
        scale: SCALE,
    };
    // 1. the least horizon that reaches the recorded state
    let t0 = Instant::now();
    let mut best: Option<Solved> = None;
    let mut seed = st.seed.clone();
    let mut tried = Vec::new();
    for horizon in (((n as f64 * MIN_FRACTION).ceil() as usize).max(1)..=n).rev() {
        let s = solve_covered(
            world,
            &mut walls,
            &st.path,
            |w| Problem {
                kernel,
                walls: w,
                x0: st.x0,
                goal: reach.clone(),
                opts: opts(),
            },
            &resample(&seed, horizon),
        );
        let pb = Problem {
            kernel,
            walls: &walls,
            x0: st.x0,
            goal: reach.clone(),
            opts: opts(),
        };
        tried.push(format!(
            "{horizon}: err {:.1e} clear {:.3}",
            pb.value(&s.plan.x),
            pb.worst(&s.plan.x, &s.plan.u)
        ));
        if !s.feasible {
            break;
        }
        seed = s.plan.u.clone();
        best = Some(s);
    }
    let generic_secs = t0.elapsed().as_secs_f64();
    let Some(best) = best else {
        return Ok(Outcome {
            line: format!(
                "{}..{}: no horizon reaches the recorded state ({}), {generic_secs:.2} s",
                st.a,
                st.b,
                tried.join("; ")
            ),
            ok: false,
            inputs: None,
        });
    };
    let pb = Problem {
        kernel,
        walls: &walls,
        x0: st.x0,
        goal: reach.clone(),
        opts: opts(),
    };
    let ng = best.plan.u.len();

    // 2. faithful: open loop, then repaired; a repair that touches falls back a tick
    let open = refly(
        world,
        &st.x0,
        &best.plan.x,
        &best.plan.inputs(st.yaw_from),
        1e-3,
    );
    let t1 = Instant::now();
    let mut rep = repair(world, &pb, &best, st.yaw_from, 100.0, 3);
    let mut used = best.clone();
    while !rep.clean() && used.plan.u.len() < n {
        let horizon = used.plan.u.len() + 1;
        used = pb.solve(&resample(&used.plan.u, horizon));
        rep = repair(world, &pb, &used, st.yaw_from, 100.0, 3);
    }
    let faithful_secs = t1.elapsed().as_secs_f64();
    let nf = rep.inputs.len();
    let end = *rep.flown.last().unwrap();
    let end_err: Vec<f64> = (0..6).map(|i| (end[i] - st.target[i]).abs()).collect();
    let pos_err = end_err[..3].iter().copied().fold(0.0, f64::max);
    let vel_err = end_err[3..].iter().copied().fold(0.0, f64::max);

    // 3. splice and verify
    let source = format!(
        "resolve: ticks {}..{} re-solved under the generic kernel (DDP, ghost walls), \
         {nf} ticks, repaired under the faithful kernel, spliced into runs/recorded.traj.jsonl, \
         flown with port",
        st.a, st.b
    );
    let (traj, err) = splice(
        rec,
        st.a,
        st.b,
        &rep.inputs,
        world,
        header(course, rec, OBJECTIVE, source),
    );
    let out = dir.join(format!("runs/resolve_{}_{}.traj.jsonl", st.a, st.b));
    traj.write(&out)?;
    let report = verify(course, world, VPlan::Trajectory(&traj));
    // the same splice without the repair: does the open-loop plan hold up?
    let (open_traj, _) = splice(
        rec,
        st.a,
        st.b,
        &best.plan.inputs(st.yaw_from),
        world,
        header(course, rec, OBJECTIVE, String::new()),
    );
    let open_report = verify(course, world, VPlan::Trajectory(&open_traj));
    let open_verdict = match open_report.timer_tick().zip(open_report.finish_tick()) {
        Some((s, f)) if open_report.ok() => format!("PASS, timer {} ticks", f - s),
        _ => format!("FAIL ({})", open_report.failures.join("; ")),
    };
    let (ticks, secant) = (
        report
            .timer_tick()
            .zip(report.finish_tick())
            .map(|(s, f)| f - s),
        report.secant_seconds().map(|s| s / 0.05),
    );

    // 4. the free crossing, for comparison
    let vb = [st.target[3], st.target[4], st.target[5]];
    let len = (vb[0] * vb[0] + vb[1] * vb[1] + vb[2] * vb[2]).sqrt();
    let normal = vb.map(|v| v / len);
    let at = (0..3).map(|i| normal[i] * st.target[i]).sum::<f64>();
    let plane_goal = Goal::Plane {
        normal,
        at,
        cap: n + 20,
    };
    let t2 = Instant::now();
    let ps = solve_covered(
        world,
        &mut walls,
        &st.path,
        |w| Problem {
            kernel,
            walls: w,
            x0: st.x0,
            goal: plane_goal.clone(),
            opts: opts(),
        },
        &st.seed,
    );
    let plane = Problem {
        kernel,
        walls: &walls,
        x0: st.x0,
        goal: plane_goal.clone(),
        opts: opts(),
    };
    let plane_secs = t2.elapsed().as_secs_f64();
    let pt = plane.value(&ps.plan.x);
    let pf = refly(
        world,
        &st.x0,
        &ps.plan.x,
        &ps.plan.inputs(st.yaw_from),
        1e-3,
    );
    let pft = if pf.clean() {
        format!("{:.3}", plane.value(&pf.flown))
    } else {
        format!("parts: contacts {:?} event {:?}", pf.contacts, pf.event)
    };
    let xe = ps.plan.x.last().unwrap();
    let off = ((xe[0] - st.target[0]).powi(2)
        + (xe[1] - st.target[1]).powi(2)
        + (xe[2] - st.target[2]).powi(2))
    .sqrt();
    let speed = |x: &X| (x[3] * x[3] + x[4] * x[4] + x[5] * x[5]).sqrt();

    let mut s = String::new();
    use std::fmt::Write;
    let _ = writeln!(s, "{}..{} ({n} recorded ticks)", st.a, st.b);
    let _ = writeln!(
        s,
        "  {OBJECTIVE}: generic {ng} ticks (saves {}), solve {generic_secs:.2} s; horizons {}",
        n - ng,
        tried.join("; ")
    );
    let _ = writeln!(
        s,
        "  open-loop faithful: max gap {:.1e}, first over 1e-3 at {:?}, contacts {:?}, event {:?}",
        open.max_gap(),
        open.first_over,
        open.contacts,
        open.event
    );
    let _ = writeln!(
        s,
        "  repaired faithful: {nf} ticks (saves {}), {} re-solves {:.2} s (all repairs {faithful_secs:.2} s), \
         end error pos {pos_err:.1e} vel {vel_err:.1e}, contacts {:?}, event {:?}",
        n - nf,
        rep.resolves,
        rep.secs,
        rep.contacts,
        rep.event
    );
    let verdict = if report.ok() { "PASS" } else { "FAIL" };
    let _ = writeln!(
        s,
        "  stitched: {verdict}, timer {} ticks (recorded {}), secant {} (recorded {:.3}), port {}; {}",
        ticks.map_or("-".into(), |t| t.to_string()),
        base.0,
        secant.map_or("-".into(), |t| format!("{t:.3}")),
        base.1,
        err.map_or("flew it all".into(), |(t, u)| format!(
            "stopped at {t}: {u:?}"
        )),
        out.display()
    );
    let _ = writeln!(s, "  stitched open loop, unrepaired: {open_verdict}");
    if !report.ok() {
        for f in &report.failures {
            let _ = writeln!(s, "    {f}");
        }
    }
    let started_past = (0..3).map(|i| normal[i] * st.x0[i]).sum::<f64>() > at;
    if started_past {
        let _ = writeln!(
            s,
            "  plane_secant (state at b free): n/a, the start is already past the plane"
        );
    } else {
        let _ = writeln!(
            s,
            "  plane_secant (state at b free): generic {pt:.3} (recorded {n}), faithful {pft}, {plane_secs:.2} s; \
             {}; its last tick {off:.1} blocks from the recorded state at b, y {:+.2}, speed {:+.3} b/t",
            if ps.feasible {
                "ghost-feasible"
            } else {
                "not ghost-feasible"
            },
            xe[1] - st.target[1],
            speed(xe) - speed(&st.target)
        );
    }
    Ok(Outcome {
        line: s,
        ok: report.ok(),
        inputs: report.ok().then_some(rep.inputs),
    })
}

fn run() -> Result<bool, String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (course, world) = Course::load(dir)?;
    let rec_path = dir.join("runs/recorded.traj.jsonl");
    let rec =
        Trajectory::read(&rec_path).map_err(|e| format!("{e} (run the `recorded` phase first)"))?;
    let base_report = verify(&course, &world, VPlan::Trajectory(&rec));
    if !base_report.ok() {
        return Err("the recorded trajectory doesn't verify".into());
    }
    let base = (
        base_report.finish_tick().unwrap() - base_report.timer_tick().unwrap(),
        base_report.secant_seconds().unwrap() / 0.05,
    );
    let mut stretches = Vec::new();
    for a in std::env::args().skip(1) {
        let (x, y) = a.split_once("..").ok_or(format!("{a}: want A..B"))?;
        let p = |s: &str| s.parse::<usize>().map_err(|e| format!("{a}: {e}"));
        stretches.push((p(x)?, p(y)?));
    }
    if stretches.is_empty() {
        stretches = DEFAULT.to_vec();
    }
    let mut ok = true;
    let mut done = Vec::new();
    for (a, b) in stretches {
        let st = match stretch(&rec, a, b) {
            Ok(s) => s,
            Err(e) => {
                println!("{a}..{b}: skipped: {e}");
                continue;
            }
        };
        let o = one(dir, &course, &world, &rec, base, &st)?;
        print!("{}", o.line);
        ok &= o.ok;
        if let Some(i) = o.inputs {
            done.push((a, b, i));
        }
    }
    // every stretch that verified, spliced into one run, latest first so that the
    // earlier stretches' ticks keep their numbers
    done.sort_by_key(|d| std::cmp::Reverse(d.0));
    let disjoint = done.windows(2).all(|w| w[1].1 <= w[0].0);
    if done.len() > 1 && disjoint {
        let mut all = rec.clone();
        for (a, b, i) in &done {
            let src = format!(
                "resolve: stretches {:?} spliced into runs/recorded.traj.jsonl",
                done.iter().map(|d| (d.0, d.1)).collect::<Vec<_>>()
            );
            let (t, _) = splice(
                &all,
                *a,
                *b,
                i,
                &world,
                header(&course, &rec, OBJECTIVE, src),
            );
            all = t;
        }
        let out = dir.join("runs/resolve_all.traj.jsonl");
        all.write(&out)?;
        let r = verify(&course, &world, VPlan::Trajectory(&all));
        let ticks = r.timer_tick().zip(r.finish_tick()).map(|(s, f)| f - s);
        println!(
            "all {} stretches spliced: {}, timer {} ticks (recorded {}), secant {} ; {}",
            done.len(),
            if r.ok() { "PASS" } else { "FAIL" },
            ticks.map_or("-".into(), |t| t.to_string()),
            base.0,
            r.secant_seconds()
                .map_or("-".into(), |s| format!("{:.3}", s / 0.05)),
            out.display()
        );
        for f in &r.failures {
            println!("    {f}");
        }
        ok &= r.ok();
    }
    Ok(ok)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("resolve: {e}");
            ExitCode::from(2)
        }
    }
}
