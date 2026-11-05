//! Phase `tas`: a plan `/etas play` can fly, from an `optimize` flight. A flight starts in
//! the air wherever its seed deployed, often on a ledge the reset doesn't put you on, so
//! this flies a seed's own start from the reset pose and joins the flight.
//!
//! - The prefix: a recorded tape that starts at the reset pose and replays under port with no
//!   server events before its deploy (`PREFIX`), up to that deploy.
//! - The join: from the deploy's state to the flight's state `join` ticks past its timer
//!   start, on the generic kernel, over horizons around the prefix tape's own. Horizons and
//!   joins are searched for the fewest timed ticks over the whole run. why timed? ticks before
//!   the timer starts cost nothing, so the least ticks to the join isn't the goal.
//! - The rest: the flight's own states as targets for `refly::track`, flown with port from
//!   rest, then `plan_from` and `verify_paths`.
//!
//! Writes `runs/tas/academy.tas`, the plan to copy to `<profile>/elytra-tas/plans/`.
//!
//! `cargo run --release -p academy --bin tas -- FLIGHT_NAME [PREFIX_TAPE]`

use std::path::Path;
use std::process::ExitCode;

use etas::course::Course;
use etas::plan::{Plan, plan_from, verify_paths};
use etas::solve::bounces::{SCALE, flight, fly, state_at};
use etas::solve::ddp::{Goal, Opts, Plan as DPlan, Problem, U, X, control, lead_of};
use etas::solve::refly::track;
use etas::solve::stretch::resample;
use etas::solve::walls::Walls;
use etas::tape::Tape;
use etas::trajectory::{Header, InputRow, Trajectory, flight_from_tape, from_tape};
use etas::verify::{Plan as VPlan, verify};
use rayon::prelude::*;
use sim::generic::Generic;

const OBJECTIVE: &str = "finish_gate_segments";
/// The default prefix: it starts at the reset pose. It lands once on the start floor and
/// jumps, so the plan has an `@react` there; 37.26's does too, and joins a tick slower.
const PREFIX: &str = "runs/seeds/34.020_20261103_22_4_run10.tas";
const TAIL: usize = 7;
const MARGIN: f64 = 0.02;
const JOINS: [usize; 6] = [15, 25, 40, 60, 80, 110];
const TRACK_WINDOWS: [usize; 4] = [24, 12, 6, 3];
/// How far past the seed's own horizon a join looks. why so far? ticks in the start area are
/// free, so a join that spends more of them diving can arrive faster.
const HORIZON_UP: usize = 60;

fn finish_goal(course: &Course, cap: usize) -> Result<Goal, String> {
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

fn opts(margin: f64) -> Opts {
    Opts {
        tol: 1e-6,
        margin,
        ..Opts::default()
    }
}

/// A flight's rows as controls and states, from tick 0.
fn controls(t: &Trajectory, from: usize, to: usize) -> Vec<U> {
    (from..to)
        .map(|k| {
            let i = &t.rows[k].input;
            control(lead_of(&flight(state_at(t, k)), i.yaw), i.pitch)
        })
        .collect()
}

fn run() -> Result<bool, String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (course, world) = Course::load(dir)?;
    let name = std::env::args().nth(1).ok_or("want a flight name")?;
    // a flight's path under `runs/`, as `relax/NAME`, or a name in `optimize/`
    let name = if name.contains('/') {
        name
    } else {
        format!("optimize/{name}")
    };
    let prefix = std::env::args().nth(2).unwrap_or(PREFIX.to_string());
    let prefix = prefix.as_str();
    let best = Trajectory::read(&dir.join(format!("runs/{name}.traj.jsonl")))?;
    let r = verify(&course, &world, VPlan::Trajectory(&best));
    let (b_timer, b_finish) = r
        .timer_tick()
        .zip(r.finish_tick())
        .ok_or("the flight has no timer or finish")?;
    println!("flight: {} timed ticks", b_finish - b_timer);

    // the prefix, from rest
    let tape = Tape::read(&dir.join(prefix))?;
    let (rest, err) = from_tape(&tape, &world, Some(&course), prefix);
    if let Some((t, u)) = err {
        println!("prefix tape: port stopped at {t}: {u:?} (only its start is used)");
    }
    let seed_flight = flight_from_tape(&tape, &world, &course, prefix, None)?;
    let deploy = seed_flight.from_tick;
    let start = state_at(&rest, deploy);
    if !start.fall_flying {
        return Err(format!("prefix: not gliding at its deploy, tick {deploy}"));
    }
    let x0 = flight(start);
    let sr = verify(&course, &world, VPlan::Trajectory(&seed_flight.traj));
    let seed_exit = sr.timer_tick().ok_or("prefix: no timer start")?;
    println!("prefix: deploy at tape tick {deploy}, exit {seed_exit} ticks later");

    let pts: Vec<[f64; 3]> = rest
        .rows
        .iter()
        .chain(best.rows.iter())
        .map(|r| r.state.position)
        .collect();
    let walls = Walls::around(&world, &pts, 6.0, 1.0);
    let kernel = Generic::default();
    let start_area = course.region("start_area")?.working.clone();

    // the join: per target tick and horizon, the exit tick it comes to
    let found: Vec<(usize, usize, usize, DPlan)> = JOINS
        .par_iter()
        .flat_map_iter(|&j| {
            let target_tick = b_timer + j;
            let target: X = flight(state_at(&best, target_tick));
            let pb = Problem {
                kernel,
                walls: &walls,
                x0,
                goal: Goal::Reach {
                    target,
                    scale: SCALE,
                },
                opts: opts(MARGIN),
            };
            let seed_n = seed_exit + j;
            let seed = controls(
                &seed_flight.traj,
                0,
                seed_n.min(seed_flight.traj.rows.len()),
            );
            let mut out = Vec::new();
            let mut u = seed;
            for n in (seed_n.saturating_sub(25)..=seed_n + HORIZON_UP).rev() {
                let s = pb.solve(&resample(&u, n));
                let loose = Problem {
                    opts: opts(MARGIN - 0.003),
                    ..pb.clone()
                };
                if !loose.feasible(&s.plan) {
                    continue;
                }
                u = s.plan.u.clone();
                let exit = s
                    .plan
                    .x
                    .iter()
                    .position(|x| !start_area.contains([x[0], x[1], x[2]]))
                    .unwrap_or(n);
                // the timed ticks up to the join; the rest is the flight's
                out.push((n - exit + (b_finish - target_tick), j, n, s.plan));
            }
            out
        })
        .collect();
    let mut found = found;
    found.sort_by_key(|f| f.0);
    for f in found.iter().take(8) {
        let e =
            f.3.x
                .iter()
                .position(|x| !start_area.contains([x[0], x[1], x[2]]))
                .unwrap_or(0);
        let v = f.3.x[e];
        println!(
            "join +{} in {} ticks: predicted timed {}, exit tick {e}, speed {:.2}",
            f.1,
            f.2,
            f.0,
            (v[3] * v[3] + v[4] * v[4] + v[5] * v[5]).sqrt()
        );
    }
    let (timed, j, _n, join) = found
        .into_iter()
        .next()
        .ok_or("no join reaches the flight")?;
    println!("join chosen: +{j}, predicted {timed} timed");

    // the whole plan, generic: the join, then the flight's rotations as controls
    let target_tick = b_timer + j;
    let end = b_finish.min(best.rows.len());
    let rest_u = controls(&best, target_tick, end);
    let u: Vec<U> = join.u.iter().chain(rest_u.iter()).copied().collect();
    let pb = Problem {
        kernel,
        walls: &walls,
        x0,
        goal: finish_goal(&course, u.len() + 40)?,
        opts: opts(MARGIN),
    };
    // the states to track are the flight's own, not a generic rollout of its rotations,
    // which drifts into walls over hundreds of ticks
    let x: Vec<X> = join
        .x
        .iter()
        .copied()
        .chain((target_tick + 1..=end).map(|t| flight(state_at(&best, t))))
        .collect();
    println!("plan: {} ticks", u.len());
    let plan = DPlan { u, x };

    // faithful, from rest
    let tracker = Problem {
        opts: opts(0.004),
        ..pb.clone()
    };
    let yaw_from = start.y_rot;
    let mut reps: Vec<_> = TRACK_WINDOWS
        .iter()
        .map(|&w| track(&world, &tracker, &plan, yaw_from, w, SCALE))
        .collect();
    let at = reps.iter().position(|r| r.clean()).unwrap_or(0);
    let rep = reps.swap_remove(at);
    println!(
        "track window {}: {} ticks, contacts {:?}, event {:?}",
        TRACK_WINDOWS[at],
        rep.inputs.len(),
        &rep.contacts[..rep.contacts.len().min(8)],
        rep.event
    );

    let mut inputs: Vec<InputRow> = rest.rows[..deploy]
        .iter()
        .map(|r| r.input.clone())
        .collect();
    let keys = rest.rows[deploy].input.keys.replace('J', "");
    let row = |u: &sim::state::Input<f32>| InputRow {
        yaw: u.yaw as f64,
        pitch: u.pitch as f64,
        keys: keys.clone(),
    };
    inputs.extend(rep.inputs.iter().map(row));
    let last = *rep.inputs.last().ok_or("empty track")?;
    inputs.extend((0..TAIL).map(|_| row(&last)));
    let mut h = Header::new(
        Some(&course),
        OBJECTIVE,
        &format!("tas: {prefix} to its deploy, then {name}"),
    );
    h.tas_header = Some(vec![("source".to_string(), format!("academy {name}"))]);
    let (traj, stopped) = fly(&rest.initial, &inputs, &[], &world, h);
    if let Some((t, u)) = stopped {
        return Err(format!("port stopped at {t}: {u:?}"));
    }
    let out = dir.join("runs/tas");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    traj.write(&out.join("academy.traj.jsonl"))?;

    let (plan, report) = plan_from(&course, &world, &traj);
    println!(
        "trajectory: {}, timer {:?}, finish {:?}{}",
        if report.ok() { "PASS" } else { "FAIL" },
        report.timer_tick(),
        report.finish_tick(),
        report
            .failures
            .first()
            .map_or(String::new(), |e| format!(": {e}"))
    );
    let file = out.join("academy.tas");
    std::fs::write(&file, plan.write()).map_err(|e| e.to_string())?;
    let read = Plan::read(&file)?;
    let mut ok = report.ok();
    for (p, r) in verify_paths(&course, &world, &read) {
        let timed = r.timer_tick().zip(r.finish_tick()).map(|(s, f)| f - s);
        println!(
            "plan path {:?}: {} timed {:?} ({})",
            p.leaf,
            if r.ok() { "PASS" } else { "FAIL" },
            timed,
            timed.map_or("-".into(), |t| format!("{:.2} s", t as f64 * 0.05))
        );
        ok &= r.ok();
    }
    println!("{}", file.display());
    Ok(ok)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("tas: {e}");
            ExitCode::from(2)
        }
    }
}
