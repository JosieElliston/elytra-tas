//! Phase `anneal`: a roughly annealed search around a flight, for partial ticks at each wall margin.
//!
//! `optimize`'s rounds stop at a local optimum: which wall a flight leans on, and the order of a tick's L-shaped move, are
//! choices no gradient crosses. So each chain moves a state at a tick off the plan, flies there and back to a later tick of
//! the plan by two reaches, solves from there to the finish, and takes the result by Metropolis on the finish's secant.
//! Temperature falls in a sawtooth and a chain may restart from the best at that margin. A move centers on a tick near a
//! wall or in the approach to one, since a glide's momentum into a tight spot is made well before it: most moves center up
//! to `APPROACH` ticks ahead of a contact, and the solve to the finish opens up to `LEAD` ticks before the move, so the
//! approach is free to change. A kind of move changes speed along the flight, with height, rather than position alone,
//! another aims past the plan's state, as `optimize`'s `carry` does, and another shifts a stretch's pitch by a few ticks, for
//! when to dive and pull out. why that one? what a dive is worth is its timing, and moved states rarely change it.
//!
//! Partial ticks are a gauge, not a result: the server counts poses, so only `verify`'s whole ticks count, but a flight's
//! secant moves long before its whole ticks do. A chain's best at a margin is flown under `track` and written as
//! `runs/relax/NAME_mMARGIN`, whatever it times.
//!
//! Each new best also goes in `runs/anneal/NAME.jsonl`, and every candidate in `runs/anneal/NAME.moves.tsv`, to read whether
//! moves find other optima or fall back to the same one: kind (p position, v momentum, s pitch shift), center, opened from, value, clearance, margin. The head up to the door is held, so the door's fraction doesn't move.
//!
//! `cargo run --release -p academy --bin anneal -- BASE NAME --margins M... [--chains N] [--secs S] [--seed S] [--focus T0 T1]...`

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use etas::course::Course;
use etas::solve::bounces::{SCALE, flight, fly, state_at};
use etas::solve::ddp::{Goal, Opts, Plan as DPlan, Problem, U, X, control, lead_of, server_hits};
use etas::solve::refly::track;
use etas::solve::stretch::resample;
use etas::solve::walls::Walls;
use etas::trajectory::{Header, InputRow, Trajectory};
use etas::verify::{Plan, verify};
use rayon::prelude::*;
use sim::generic::Generic;
use world::World;

/// A plan's clearance is judged this far under its margin: the augmented Lagrangian closes on it from below.
const LOOSE: f64 = 0.002;
/// How far off a reach may end, by [`Goal::Reach`]'s value. Only a seed: the solve to the finish is the judge.
const MISS: f64 = 0.1;
/// Temperature in ticks, from high to low over a cycle of moves.
const T_HIGH: f64 = 0.04;
const T_LOW: f64 = 0.002;
const CYCLE: usize = 60;
/// How much a new best has to gain to be flown. why not any? a track takes as long as many moves.
const FLY_GAIN: f64 = 0.01;
const TRACK_WINDOWS: [usize; 4] = [24, 12, 6, 3];
const TRACK_MARGIN: f64 = 0.004;
/// How far ahead of a contact a move may center, and how far before a move the solve to the finish may open, in ticks.
const APPROACH: usize = 120;
const LEAD: usize = 150;
/// Ticks flown past the finish on the last rotation, as in `optimize`.
const TAIL: usize = 7;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: usize, b: usize) -> usize {
        a + (self.next() * (b - a) as f64) as usize
    }
    fn normal(&mut self) -> f64 {
        let (a, b) = (self.next().max(1e-12), self.next());
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

struct Base {
    rec: Trajectory,
    a: usize,
    f: usize,
}

struct Best {
    margin: f64,
    plan: DPlan,
    value: f64,
    flown: f64,
}

fn goal(course: &Course, cap: usize) -> Result<Goal, String> {
    let f = &course.region("finish")?.working;
    Ok(Goal::Gate {
        normal: [0.0, 0.0, 1.0],
        at: f.min[2].ok_or("finish has no z_min")?,
        cap,
        lo: [f.min[0].unwrap_or(f64::NEG_INFINITY), f.min[1].unwrap_or(f64::NEG_INFINITY), f64::NEG_INFINITY],
        hi: f.max.map(|m| m.unwrap_or(f64::INFINITY)),
    })
}

fn opts(margin: f64) -> Opts {
    Opts { tol: 1e-6, margin, server: true, ..Opts::default() }
}

fn stiff(margin: f64) -> Opts {
    Opts { c0: 1e3, iters: 300, ..opts(margin) }
}

fn clear(pb: &Problem, plan: &DPlan, margin: f64) -> bool {
    let n = plan.x.len() - 1;
    n > 0 && pb.crossed(&plan.x[n - 1], &plan.x[n]) && pb.worst(&plan.x, &plan.u) >= margin - LOOSE
}

/// Ticks of the plan within `near` of a wall.
fn tight(pb: &Problem, plan: &DPlan, near: f64) -> Vec<usize> {
    (0..plan.u.len().saturating_sub(1))
        .filter(|&k| pb.worst(&plan.x[k..=k + 1], &plan.u[k..k + 1]) < near)
        .collect()
}

/// The plan with one state moved: there and back by two reaches, then a solve to the finish. None if a reach misses.
#[allow(clippy::too_many_arguments)]
fn propose(walls: &Walls, gate: &Problem, plan: &DPlan, margin: f64, from: usize, center: usize, l1: usize, l2: usize, target: X, end: X) -> Option<DPlan> {
    let n = plan.u.len();
    let (a, b) = (center - l1, (center + l2).min(n - 1));
    let reach = |x0: X, to: X, seed: &[U]| {
        let pb = Problem {
            kernel: Generic::default(),
            walls,
            x0,
            goal: Goal::Reach { target: to, scale: SCALE },
            opts: Opts { tol: MISS, ..opts(margin) },
        };
        let s = pb.solve(seed);
        (pb.value(&s.plan.x) <= MISS).then_some(s.plan)
    };
    let p1 = reach(plan.x[a], target, &resample(&plan.u[a..center], l1))?;
    let p2 = reach(*p1.x.last().unwrap(), end, &resample(&plan.u[center..b], b - center))?;
    let mut tail = p1.u;
    tail.extend(p2.u);
    tail.extend_from_slice(&plan.u[b..]);
    let mut whole = plan.u[from..a].to_vec();
    whole.extend(tail);
    let pb = Problem { x0: plan.x[from], opts: stiff(margin), ..gate.clone() };
    let s = pb.solve(&whole);
    let mut x = plan.x[..from].to_vec();
    x.extend_from_slice(&s.plan.x);
    let mut u = plan.u[..from].to_vec();
    u.extend(s.plan.u);
    Some(DPlan { u, x })
}

/// The plan with a stretch's pitch moved `shift` ticks later, then a solve to the finish.
fn shifted(gate: &Problem, plan: &DPlan, margin: f64, from: usize, a: usize, b: usize, shift: i32) -> DPlan {
    let n = plan.u.len() as i32;
    let mut u = plan.u.clone();
    for k in a..b {
        u[k][1] = plan.u[(k as i32 - shift).clamp(0, n - 1) as usize][1];
    }
    let pb = Problem { x0: plan.x[from], opts: stiff(margin), ..gate.clone() };
    let s = pb.solve(&u[from..]);
    let mut x = plan.x[..from].to_vec();
    x.extend_from_slice(&s.plan.x);
    let mut w = plan.u[..from].to_vec();
    w.extend(s.plan.u);
    DPlan { u: w, x }
}

/// The plan flown under the faithful kernel, verified and written; its timed ticks, the flown secant and a report line.
#[allow(clippy::too_many_arguments)]
fn fly_it(dir: &Path, course: &Course, world: &World, walls: &Walls, base: &Base, gate: &Problem, plan: &DPlan, margin: f64, name: &str) -> Result<(f64, String), String> {
    let a = base.a;
    let tracker = Problem { opts: Opts { margin: TRACK_MARGIN, ..opts(margin) }, ..gate.clone() };
    let yaw_from = state_at(&base.rec, a).y_rot;
    let mut reps: Vec<_> = TRACK_WINDOWS.iter().map(|&w| track(world, &tracker, plan, yaw_from, w, SCALE)).collect();
    let at = reps.iter().position(|r| r.clean()).unwrap_or(0);
    let rep = reps.swap_remove(at);
    let mut inputs: Vec<InputRow> = base.rec.rows[..a].iter().map(|r| r.input.clone()).collect();
    let keys = base.rec.rows[a].input.keys.replace('J', "");
    let row = |u: &sim::state::Input<f32>| InputRow { yaw: u.yaw as f64, pitch: u.pitch as f64, keys: keys.clone() };
    inputs.extend(rep.inputs.iter().map(row));
    let last = *rep.inputs.last().ok_or("empty track")?;
    inputs.extend((0..TAIL).map(|_| row(&last)));
    let mut h = Header::new(Some(course), "finish_gate_segments", &format!("anneal: {name} from tick {a}"));
    h.tas_header = base.rec.header.tas_header.clone();
    let arrivals: Vec<_> = base.rec.arrivals.iter().filter(|x| x.tick <= a).copied().collect();
    let (traj, _) = fly(&base.rec.initial, &inputs, &arrivals, world, h);
    let r = verify(course, world, Plan::Trajectory(&traj));
    let timed = r.timer_tick().zip(r.finish_tick()).map(|(s, e)| e - s);
    let xs: Vec<X> = traj.rows.iter().map(|w| flight(&w.state)).collect();
    let hits: Vec<usize> = server_hits(walls, &xs[a..r.finish_tick().unwrap_or(xs.len()).min(xs.len())]).iter().map(|h| h.0 + a + 1).collect();
    let secant = r.secant_seconds().map_or(f64::NAN, |s| s / 0.05);
    let clearance = r.clearance.as_ref().map_or(f64::NAN, |c| c.min);
    if r.ok() {
        traj.write(&dir.join("runs/relax").join(format!("{name}.traj.jsonl")))?;
    }
    Ok((
        secant,
        format!(
            "{{\"name\":\"{name}\",\"margin\":{margin},\"generic\":{:.4},\"timed\":{},\"secant\":{secant:.4},\"clearance\":{clearance:.4},\"server_hits\":{hits:?},\"ok\":{},\"window\":{}}}",
            gate.value(&plan.x),
            timed.map_or("null".into(), |t: usize| t.to_string()),
            r.ok(),
            TRACK_WINDOWS[at],
        ),
    ))
}

fn run() -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (course, world) = Course::load(dir)?;
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let (base_name, name) = (args.remove(0), args.remove(0));
    let (mut margins, mut focus, mut chains, mut secs, mut seed) = (Vec::new(), Vec::new(), 4usize, 3600.0f64, 1u64);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--margins" => {
                i += 1;
                while i < args.len() && !args[i].starts_with("--") {
                    margins.push(args[i].parse::<f64>().map_err(|e| e.to_string())?);
                    i += 1;
                }
                continue;
            }
            "--focus" => {
                focus.push((args[i + 1].parse::<usize>().unwrap(), args[i + 2].parse::<usize>().unwrap()));
                i += 2;
            }
            "--chains" => { chains = args[i + 1].parse().unwrap(); i += 1; }
            "--secs" => { secs = args[i + 1].parse().unwrap(); i += 1; }
            "--seed" => { seed = args[i + 1].parse().unwrap(); i += 1; }
            o => return Err(format!("unknown argument {o}")),
        }
        i += 1;
    }
    if margins.is_empty() {
        return Err("--margins M...".into());
    }
    let rec = Trajectory::read(&dir.join(format!("runs/relax/{base_name}.traj.jsonl")))?;
    let start = &course.region("start_area")?.working;
    let at = course.region("finish")?.working.min[2].ok_or("finish has no z_min")?;
    let pos = |t: usize| state_at(&rec, t).position;
    let n = rec.rows.len();
    let timer = start.exit_move((0..n).map(pos)).ok_or("no timer")?;
    let f = (timer..n).find(|&t| pos(t - 1)[2] <= at && pos(t)[2] > at).ok_or("no finish")?;
    let a = timer - 1;
    let pts: Vec<[f64; 3]> = rec.rows.iter().map(|x| x.state.position).collect();
    let walls = Walls::around(&world, &pts, 6.0, 1.0);
    let seed_u: Vec<U> = (a..f)
        .map(|t| {
            let i = &rec.rows[t].input;
            control(lead_of(&flight(state_at(&rec, t)), i.yaw), i.pitch)
        })
        .collect();
    let x0 = flight(state_at(&rec, a));
    let base = Base { rec, a, f };
    let cap = base.f - a + 80;
    let g = goal(&course, cap)?;
    let gate_at = |m: f64| Problem { kernel: Generic::default(), walls: &walls, x0, goal: g.clone(), opts: stiff(m) };

    // each margin's start, raised from the last, so a margin past what the base has is reached by steps
    margins.sort_by(|p, q| p.partial_cmp(q).unwrap());
    let mut u = seed_u.clone();
    let mut bests = Vec::new();
    for &m in &margins {
        let pb = gate_at(m);
        let t0 = Instant::now();
        let s = pb.solve(&u);
        let (v, w) = (pb.value(&s.plan.x), pb.worst(&s.plan.x, &s.plan.u));
        println!("start margin {m}: value {v:.4} clear {w:.4} in {:.1} s", t0.elapsed().as_secs_f64());
        if !clear(&pb, &s.plan, m) {
            println!("  no start at margin {m}; skipped");
            continue;
        }
        u = s.plan.u.clone();
        bests.push(Mutex::new(Best { margin: m, value: v, plan: s.plan, flown: f64::INFINITY }));
    }
    if bests.is_empty() {
        return Err("no margin has a start".into());
    }
    std::fs::create_dir_all(dir.join("runs/anneal")).map_err(|e| e.to_string())?;
    let log = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("runs/anneal/{name}.jsonl"))).map_err(|e| e.to_string())?);
    let moves_log = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("runs/anneal/{name}.moves.tsv"))).map_err(|e| e.to_string())?);
    let t_all = Instant::now();
    (0..chains).into_par_iter().for_each(|c| {
        let slot = &bests[c % bests.len()];
        let (margin, mut plan, mut value) = {
            let b = slot.lock().unwrap();
            (b.margin, b.plan.clone(), b.value)
        };
        let gate = gate_at(margin);
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (c as u64 + 1).wrapping_mul(0xD134_2543_DE82_EF95));
        let (mut moves, mut took, mut failed) = (0usize, 0usize, 0usize);
        while t_all.elapsed().as_secs_f64() < secs {
            let phase = (moves % CYCLE) as f64 / CYCLE as f64;
            let temp = T_HIGH * (T_LOW / T_HIGH).powf(phase);
            if moves % CYCLE == 0 && moves > 0 && rng.next() < 0.5 {
                let b = slot.lock().unwrap();
                (plan, value) = (b.plan.clone(), b.value);
            }
            moves += 1;
            let n = plan.u.len();
            let near = tight(&gate, &plan, margin + 0.01);
            let (l1, l2) = (rng.range(6, 50), rng.range(6, 50));
            // ticks in a run, against the plan's own count from `a`
            let center = if !focus.is_empty() {
                let &(t0, t1) = &focus[rng.range(0, focus.len())];
                rng.range(t0.saturating_sub(a), t1.saturating_sub(a).min(n))
            } else if rng.next() < 0.75 && !near.is_empty() {
                // momentum: mostly ahead of the contact
                let c = near[rng.range(0, near.len())];
                if rng.next() < 0.7 { c.saturating_sub(rng.range(5, APPROACH)) } else { c }
            } else {
                rng.range(0, n)
            };
            let center = center.clamp(l1, n - 2);
            let size = 0.05 + 0.6 * rng.next().powi(2);
            let mut target = plan.x[center];
            let pick = rng.next();
            let kind = if pick < 0.3 { 's' } else if pick < 0.6 { 'v' } else { 'p' };
            if kind == 'v' {
                // momentum: faster or slower along the flight, paid or banked in height
                let v = [target[3], target[4], target[5]];
                let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(0.1);
                let dv = 0.1 * size * rng.normal();
                for k in 0..3 {
                    target[k + 3] += dv * v[k] / speed;
                }
                target[1] -= speed * dv * (1.0 + rng.next());
            } else {
                for k in 0..3 {
                    target[k] += size * rng.normal();
                    target[k + 3] += 0.1 * size * rng.normal();
                }
            }
            let b = (center + l2).min(n - 1);
            // half the time, aim past the plan's state at the end, as `carry` does
            let gain = if rng.next() < 0.5 { 0.3 * rng.next() } else { 0.0 };
            let end: X = std::array::from_fn(|k| plan.x[b][k] * (1.0 - gain) + plan.x[(b + 1).min(n)][k] * gain);
            let from = (center - l1).saturating_sub(rng.range(0, LEAD));
            let got = if kind == 's' {
                let shift = [-6, -4, -3, -2, -1, 1, 2, 3, 4, 6][rng.range(0, 10)];
                Some(shifted(&gate, &plan, margin, from, center - l1, b, shift))
            } else {
                propose(&walls, &gate, &plan, margin, from, center, l1, b - center, target, end)
            };
            let Some(new) = got else {
                failed += 1;
                let _ = writeln!(moves_log.lock().unwrap(), "{kind}\t{}\t{}\tmiss\t\t{margin}", center + a, from + a);
                continue;
            };
            let v = gate.value(&new.x);
            let _ = writeln!(moves_log.lock().unwrap(), "{kind}\t{}\t{}\t{v:.5}\t{:.4}\t{margin}", center + a, from + a, gate.worst(&new.x, &new.u));
            if !clear(&gate, &new, margin) {
                failed += 1;
                continue;
            }
            if v < value || rng.next() < (-(v - value) / temp).exp() {
                (plan, value) = (new, v);
                took += 1;
            }
            let mut b = slot.lock().unwrap();
            if value < b.value {
                b.value = value;
                b.plan = plan.clone();
                println!(
                    "[{:.0} s] m{margin} chain {c} move {moves} center t{}: best {value:.4} (T {temp:.4}, took {took}, failed {failed})",
                    t_all.elapsed().as_secs_f64(),
                    center + a,
                );
                if value + FLY_GAIN < b.flown {
                    b.flown = value;
                    let p = b.plan.clone();
                    drop(b);
                    match fly_it(dir, &course, &world, &walls, &base, &gate, &p, margin, &format!("{name}_m{margin}")) {
                        Ok((_, line)) => {
                            println!("  {line}");
                            let _ = writeln!(log.lock().unwrap(), "{{\"t\":{:.0},\"value\":{value:.5},\"flown\":{line}}}", t_all.elapsed().as_secs_f64());
                        }
                        Err(e) => println!("  fly failed: {e}"),
                    }
                }
            }
        }
        println!("chain {c} m{margin}: {moves} moves, took {took}, failed {failed}, at {value:.4}");
    });
    for b in &bests {
        let b = b.lock().unwrap();
        println!("margin {}: best {:.4}", b.margin, b.value);
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("anneal: {e}");
        std::process::exit(1);
    }
}
