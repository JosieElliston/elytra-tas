//! Phase `dome`: how fast the start could be, taking the player to be anywhere under the roof.
//!
//! The timer doesn't run in the room, so a start is judged only by how it leaves: the value of a state
//! at the door. Ticks to the door are free.
//!
//! - `views`: the start room, cut open, from the side, the front and above, with the roof map.
//! - `samples OUT LO.. HI.. N [FIRST]`: exit states over a box, each worth the fewest ticks to merge
//!   into the reference at `t121` plus the reference from there. why merge? a rollout of the whole
//!   course is chaotic: a solve from a state off the reference ends inside a wall (academy,
//!   2026-11-05: infeasible from a 0.1 b/t step, at any continuation). `dome_fit.py` fits them.
//! - `starts`: every spot under the roof against the fit, ways down seeded by funneling the velocity.
//! - `results`: the roof colored by value, and the best ways down.
//! - `flights N`: the best `N` as `runs/hybrid/dome_K`, for `optimize --hybrid --open-top dome_`.
//! - `global`, `frontier`: checks owing nothing to a solve or a seed. A search over rotations from
//!   several starts, and from the apex with and without walls, for the table and for raw speeds.
//! - `slab`: the human route, off the slab at (-25.5, 283.5, 227.5): jump, sprint in the air, free-fall,
//!   deploy, over a grid of those timings, flown under port and solved against the table.
//! - `slabflights N [FILE PREFIX]`: the best `N` of `runs/dome/slab.json` or `FILE` as `runs/hybrid/slab_K`
//!   or `PREFIX`K, for `optimize --hybrid --server slab_`.
//! - `slabcheck NAME...`: flights in `runs/relax`, by `verify` and the server's box.
//! - `slabdive`: the same late pull-out from the TAS's climb, with no landing.
//! - `slabtime`: the slab's tick timings, every tick: run-up, W held after the jump, a sprint south once
//!   past the lip, a sprint north, deploy, with and without touching the corner at y 280; then spot,
//!   headings and keys around the best. `slabflights N runs/dome/slabtime.json tick_` flies them.
//!
//! As of 2026-11-05 05:50 EST, against the 537 tick TAS: every spot under the roof leaves by about the
//! same state, y 205.9, v (-0.2, -0.72, -1.55), and the best verify at 535. With no walls at all the
//! table reads 2 ticks better than in the room: a pull-out out of a dive moves down speed to forward
//! about 1:1 and drag takes the rest, so more height buys little. Contact isn't modeled.
//!
//! As of 2026-11-05 22:40 EST, the slab beats that, standing on it to start: 532 and 533 timed under `verify`
//! against the 535 TAS, for a sprint on from air tick 24-30 and a deploy at air tick 50-51, y 221. Run-up,
//! spot and yaw barely matter; no sprint reads 5 ticks worse. `slabdive` found no way to it without a
//! landing (table 538.7, the TAS 538.0, the slab 535.2). No plan flies it yet: the flights start on the
//! slab, not at the reset pose, and getting there means landing, so the server's glide-off comes at a tick
//! that depends on ping.
//!
//! As of 2026-11-05 23:50 EST, by `slabtime` (table 535.21 to 534.67): 24 of 40 flights verify at 532, 22 of
//! them clear of the server's box, against 1 of 8 for `slab`; none at 531, and a second `optimize` pass on 8
//! gained nothing. What pays is a late sprint from further back: after the lip, 10-18 ticks of S, sprint on
//! at air tick 20-25 from z 226.4-226.8 (the lip's edge; 225.1 without S), deploy at air tick 50, z 222. The
//! start spot doesn't: the three tried, z 227.9 to 228.69, read the same, since S takes each back to there.
//! Nothing drifts further south under the slab, though the shaft is open to z 230 there. Headings and the
//! deploy tick's rotation found nothing either.
//!
//! As of 2026-11-06 00:30 EST, adding a sprint north on a 45 degree strafe (2% more top speed) and a sprint
//! south: table 534.55, and of 40 flights one at 530, two at 531, thirty at 532, seven at 533. The start is
//! still about 3 ticks of that: `optimize` found the 530's other 2 at 160-280 ticks past the door, and a
//! near twin of its start got 532. The strafe north is what reads better; back keys and touching the corner
//! at y 280 read the same, though the 530 and 531s all miss it, moving south.
//!
//! `cargo run --release -p academy --bin dome -- PHASE`

use std::io::Write;
use std::f64::consts::PI;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use etas::course::Course;
use etas::solve::bounces::{SCALE, flight, fly, state_at};
use etas::solve::ddp::{Goal, Opts, Plan, Problem, U, X, control, input_at, lead_of, step};
use etas::solve::gate::{Gate, ValueTable};
use etas::solve::stretch::resample;
use etas::solve::walls::Walls;
use etas::trajectory::{InputRow, Trajectory};
use sim::generic::Generic;
use view::color::{RED, WHITE, YELLOW};
use view::{Camera, Marker, Ramp, Scene, Stroke, WorldStyle, tools};
use world::{Aabb, World};

const REFERENCE: &str = "runs/tas/academy.traj.jsonl";
/// Where the timer starts, for a flight heading north out of the door.
const EXIT_Z: f64 = 205.0;
/// Bounds the flood of the room stays in; z stops at the doorway's plane.
const LO: [i32; 3] = [-70, 190, 205];
const HI: [i32; 3] = [20, 300, 260];
const SEED: [i32; 3] = [-27, 240, 217];

fn solid(w: &World, c: [i32; 3]) -> bool {
    let p = |i: usize| c[i] as f64;
    w.collision_boxes(Aabb {
        min: [p(0), p(1), p(2)],
        max: [p(0) + 1.0, p(1) + 1.0, p(2) + 1.0],
    })
    .map_or(true, |b| !b.is_empty())
}

/// The air cells joined to the seed, per column the top and bottom of its highest run.
pub fn flood(w: &World) -> Vec<[i32; 3]> {
    let mut seen = std::collections::HashSet::new();
    let mut q = vec![SEED];
    let mut out = Vec::new();
    while let Some(c) = q.pop() {
        if (0..3).any(|i| c[i] < LO[i] || c[i] >= HI[i]) || !seen.insert(c) || solid(w, c) {
            continue;
        }
        out.push(c);
        for d in [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]] {
            q.push([c[0] + d[0], c[1] + d[1], c[2] + d[2]]);
        }
    }
    out
}

fn views(dir: &Path, course: &Course, w: &World) -> Result<(), String> {
    let cells = flood(w);
    let lo: [i32; 3] = std::array::from_fn(|i| cells.iter().map(|c| c[i]).min().unwrap());
    let hi: [i32; 3] = std::array::from_fn(|i| cells.iter().map(|c| c[i]).max().unwrap());
    println!("room: {} air cells, {lo:?} ..= {hi:?}", cells.len());
    let mut top: std::collections::BTreeMap<(i32, i32), (i32, i32)> = Default::default();
    for c in &cells {
        let e = top.entry((c[2], c[0])).or_insert((c[1], c[1]));
        e.0 = e.0.max(c[1]);
        e.1 = e.1.min(c[1]);
    }
    print!("roof y-200 per column (rows z, high z first; cols x):\n      ");
    for x in lo[0]..=hi[0] {
        print!("{:3}", x.rem_euclid(100));
    }
    println!();
    for z in (lo[2]..=hi[2]).rev() {
        print!("{z:5} ");
        for x in lo[0]..=hi[0] {
            match top.get(&(z, x)) {
                Some(t) => print!("{:3}", t.0 - 200),
                None => print!("  ."),
            }
        }
        println!();
    }
    print!("floor y-190:\n");
    for z in (lo[2]..=hi[2]).rev() {
        print!("{z:5} ");
        for x in lo[0]..=hi[0] {
            match top.get(&(z, x)) {
                Some(t) => print!("{:3}", t.1 - 190),
                None => print!("  ."),
            }
        }
        println!();
    }
    let rec = Trajectory::read(&dir.join("runs/tas/academy.traj.jsonl"))?;
    let path: Vec<[f64; 3]> = std::iter::once(rec.initial.position)
        .chain(rec.rows.iter().map(|r| r.state.position))
        .take(90)
        .collect();
    let start = &course.region("start_area")?.working;
    println!("{:?}", start);
    let out = dir.join("runs/dome");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let scene = Scene::new(w)?;
    let box_pts: Vec<[f64; 3]> = vec![
        [lo[0] as f64 - 2.0, lo[1] as f64 - 2.0, lo[2] as f64 - 14.0],
        [hi[0] as f64 + 3.0, hi[1] as f64 + 3.0, hi[2] as f64 + 3.0],
    ];
    let shot = |name: &str, cam: Camera, style: WorldStyle, (pw, ph): (u32, u32)| -> Result<(), String> {
        let mut f = scene.frame(&cam, pw, ph);
        f.style(style);
        f.polyline(&path, Stroke::new(YELLOW, 2.5).occluded(0.5));
        for b in [(*start).clone()] {
            let m: [f64; 3] = std::array::from_fn(|i| b.min[i].unwrap_or(0.0));
            let n: [f64; 3] = std::array::from_fn(|i| b.max[i].unwrap_or(0.0));
            f.aabb(m, n, Stroke::new(RED, 1.5).occluded(0.6), None);
        }
        f.point(path[0], Marker::circle(WHITE, 8.0));
        tools::rulers(&mut f);
        tools::compass(&mut f, None);
        tools::title(&mut f, name);
        f.finish().save(out.join(format!("{name}.png")))
    };
    let mid = (lo[0] + hi[0]) as f64 / 2.0 + 0.5;
    shot("side_cut_x", Camera::side(&box_pts, 90.0), WorldStyle::new().slice(0, mid - 40.0, mid), (1100, 1200))?;
    let midz = (lo[2] + hi[2]) as f64 / 2.0 + 0.5;
    shot("front_cut_z", Camera::side(&box_pts, 180.0), WorldStyle::new().slice(2, midz - 40.0, midz), (1100, 1200))?;
    shot("top_cut", Camera::top(&box_pts), WorldStyle::new().below_y(hi[1] as f64 + 0.5), (1100, 900))?;
    let eye = [mid + 60.0, 300.0, midz + 60.0];
    shot("persp", Camera::look_at(eye, [mid, 240.0, midz]).fov(50.0), WorldStyle::new().slice(0, mid - 60.0, mid).below_y(hi[1] as f64 + 40.0), (1200, 900))?;
    Ok(())
}

const MARGIN: f64 = 0.02;
const LOOSE: f64 = 0.003;
/// Infeasible horizons tried below the least that reached, and how much under the seed's a merge may go.
const PATIENCE: usize = 2;
const MIN_FRACTION: f64 = 0.35;
/// [`guided`]'s seeds per stretch: ticks a control is held to judge it, and the price of a block off the path.
const SEEDS: [(usize, f64); 3] = [(10, 0.3), (6, 0.3), (16, 1.0)];
/// How far past the plane a sampled exit state starts, ticks. why one value? the table's `tau` axis
/// is worth a fraction of a tick, against ticks a block per tick of speed.
const TAU: f64 = 0.5;
/// The reference's tick (past its exit) a sampled state merges into: the hole at `t121`, where
/// everything goes through one place and the flight has already spent what a fast exit gives it.
const MERGE_T: usize = 101;
/// Columns whose roof is at least this high are under the cone, above the tower's top floor.
const CONE_FROM: i32 = 283;
/// The clearance a spot under the roof keeps.
const ROOF_CLEAR: f64 = 0.03;
/// The door's middle, along x.
const DOOR_X: f64 = -26.5;
/// The speed of a state called at rest, blocks per tick.
const REST: f64 = 0.01;
/// The most ticks a seed for the way down gets.
const HEAD_CAP: usize = 150;
/// Sweeps in which each spot tries its better neighbors' controls, and how many neighbors it looks over.
const SWEEPS: usize = 2;
const NEAR: usize = 12;
/// The side of the room across from the door, where a glide away is headed.
const FAR_Z: f64 = 228.0;
/// How many of the funnel's rollouts go on to a solve.
const PULL_SEEDS: usize = 6;
/// How far apart, in blocks, the starts `flights` picks are.
const APART: f64 = 5.0;
/// `results`: the ticks the roof's colors span, and how many ways down it draws.
const VIEW_RANGE: f64 = 8.0;
const TOP_DRAWN: usize = 6;
/// The height the roof map is drawn at, over the cone.
const MAP_Y: f64 = 303.0;
/// `global`'s search: ticks a rotation is held and how many there are, what a crash reads, restarts
/// per start, and rounds, population and elite of each.
const KNOT: usize = 8;
const KNOTS: usize = 16;
const CRASH: f64 = 1000.0;
const RESTARTS: usize = 3;
const CEM_ROUNDS: usize = 40;
const CEM_POP: usize = 300;
const CEM_ELITE: usize = 30;

fn opts() -> Opts {
    Opts { tol: 1e-6, margin: MARGIN, ..Opts::default() }
}

fn solved(pb: &Problem, plan: &Plan) -> bool {
    let loose = Problem { opts: Opts { margin: MARGIN - LOOSE, ..pb.opts.clone() }, ..pb.clone() };
    loose.feasible(plan)
}

/// The reference flight from its exit on: states, controls, and where it stands.
struct Reference {
    rec: Trajectory,
    exit: usize,
    x: Vec<X>,
    u: Vec<U>,
}

fn reference(dir: &Path, course: &Course) -> Result<Reference, String> {
    reference_from(dir, course, REFERENCE)
}

fn reference_from(dir: &Path, course: &Course, path: &str) -> Result<Reference, String> {
    let rec = Trajectory::read(&dir.join(path))?;
    let exit = (1..rec.rows.len()).find(|&t| state_at(&rec, t).position[2] < EXIT_Z).ok_or("no exit")?;
    let fin = course.region("finish")?.working.min[2].ok_or("no finish plane")?;
    let finish = (exit..rec.rows.len())
        .find(|&t| state_at(&rec, t - 1).position[2] <= fin && state_at(&rec, t).position[2] > fin)
        .ok_or("no finish")?;
    let x = (exit..=finish).map(|t| flight(state_at(&rec, t))).collect();
    let u = (exit..finish)
        .map(|t| {
            let i = &rec.rows[t].input;
            control(lead_of(&flight(state_at(&rec, t)), i.yaw), i.pitch)
        })
        .collect();
    Ok(Reference { rec, exit, x, u })
}

/// A first guess along `path` from `x0` that stays out of walls: per tick, the control that, held
/// for `ahead` ticks, gets furthest along the path, less `cross` per block off it, and hits nothing
/// for longest. As in `routes`.
fn guided(walls: &Walls, x0: &X, path: &[[f64; 3]], ahead: usize, cross: f64, cap: usize) -> Vec<U> {
    let k = Generic::default();
    let d = |a: &[f64; 3], b: &[f64; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    let mut arc = vec![0.0];
    for w in path.windows(2) {
        arc.push(arc.last().unwrap() + d(&w[0], &w[1]));
    }
    let nearest = |p: &[f64; 3], from: usize, span: usize| -> usize {
        (from..(from + span).min(path.len())).min_by(|&a, &b| d(&path[a], p).total_cmp(&d(&path[b], p))).unwrap_or(from)
    };
    let mut x = *x0;
    let mut at = 0usize;
    let mut u = Vec::new();
    while u.len() < cap && at + 1 < path.len() {
        let mut best = (f64::NEG_INFINITY, [0.0, 0.0]);
        for lead_i in -6i32..=6 {
            for pitch in [-30.0f64, -10.0, 0.0, 10.0, 20.0, 35.0, 55.0, 80.0] {
                let c = [lead_i as f64 * 0.2, pitch.to_radians()];
                let mut s = x;
                let mut j = at;
                let mut score = f64::NEG_INFINITY;
                let mut clear = 0;
                for t in 0..ahead {
                    let (n, corners) = step(&k, &s, c);
                    if corners.windows(2).any(|l| walls.clearance(l[0], l[1]).0 < 0.05) {
                        break;
                    }
                    s = n;
                    let p = [s[0], s[1], s[2]];
                    j = nearest(&p, j, 16);
                    clear = t + 1;
                    score = arc[j] - cross * d(&path[j], &p);
                }
                let total = score - 4.0 * (ahead - clear) as f64;
                if total > best.0 {
                    best = (total, c);
                }
            }
        }
        x = step(&k, &x, best.1).0;
        let p = [x[0], x[1], x[2]];
        at = nearest(&p, at, 16);
        u.push(best.1);
    }
    if u.is_empty() {
        u.push([0.0, 0.0]);
    }
    u
}

/// The fewest ticks from `x0` to `target`, from `seed`'s length down, or up to `up` more if that fails.
fn reach(walls: &Walls, x0: X, target: X, seed: &[U], up: usize) -> Option<Plan> {
    let pb = Problem { kernel: Generic::default(), walls, x0, goal: Goal::Reach { target, scale: SCALE }, opts: opts() };
    let n0 = seed.len();
    let mut best = None;
    let mut u = seed.to_vec();
    for n in n0..=n0 + up {
        let s = pb.solve(&resample(&u, n));
        if solved(&pb, &s.plan) {
            best = Some(s.plan);
            break;
        }
        u = s.plan.u;
    }
    let mut best: Plan = best?;
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

/// From `s`, the fewest ticks to the reference's state `t` ticks past its exit. Seeded from the
/// reference's own controls, whose leads hold up for a state near it, and from paths along it.
fn merge(walls: &Walls, r: &Reference, s: &X, t: usize) -> Option<Plan> {
    let path: Vec<[f64; 3]> = r.x[..=t].iter().map(|x| [x[0], x[1], x[2]]).collect();
    let mut seeds: Vec<Vec<U>> = vec![r.u[..t].to_vec()];
    for (ahead, cross) in SEEDS {
        seeds.push(guided(walls, s, &path, ahead, cross, t + 40));
    }
    let mut best: Option<Plan> = None;
    for seed in seeds {
        // one that already beats this can't help: its horizon starts past the best
        let seed = match &best {
            Some(b) if seed.len() >= b.u.len() => resample(&seed, b.u.len() - 1),
            _ => seed,
        };
        if let Some(p) = reach(walls, *s, r.x[t], &seed, 6)
            && best.as_ref().is_none_or(|b| p.u.len() < b.u.len())
        {
            best = Some(p);
        }
    }
    best
}

/// The radical inverse of `i` in `base`: a low-discrepancy sequence, so a few hundred samples
/// spread over five axes without a random generator.
fn halton(mut i: usize, base: usize) -> f64 {
    let (mut f, mut r) = (1.0, 0.0);
    while i > 0 {
        f /= base as f64;
        r += f * (i % base) as f64;
        i /= base;
    }
    r
}

/// The exit gate's plane state with table coordinates `[x, y, vx, vy, vz]`, `TAU` ticks past it.
fn exit_state(c: &[f64; 5]) -> X {
    exit_gate().state(&[c[0], c[1], c[2], c[3], c[4], TAU])
}

fn exit_gate() -> Gate {
    Gate { axis: 2, sign: -1.0, at: -EXIT_Z, lo: [-34.0, 195.0], hi: [-18.0, 212.0] }
}

#[derive(Serialize, Deserialize)]
struct Sample {
    c: [f64; 5],
    /// Ticks to the merge, its tick on the reference, and the timed ticks that makes.
    n: usize,
    t: usize,
    value: f64,
}

/// `samples OUT LO.. HI.. N [FIRST]`: the value of exit states spread over a box, by merging.
fn samples(dir: &Path, course: &Course, w: &World, args: &[String]) -> Result<(), String> {
    let r = reference(dir, course)?;
    let v: Vec<f64> = args[1..].iter().map(|a| a.parse::<f64>().map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
    let (lo, hi): ([f64; 5], [f64; 5]) = (std::array::from_fn(|i| v[i]), std::array::from_fn(|i| v[5 + i]));
    let (n, first) = (v[10] as usize, v.get(11).map_or(1, |f| *f as usize));
    let pts: Vec<[f64; 3]> = r.rec.rows.iter().map(|x| x.state.position).collect();
    let walls = Walls::around(w, &pts, 8.0, 1.0);
    let total = r.x.len() - 1;
    let file = dir.join("runs/dome").join(&args[0]);
    let out = std::sync::Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(&file).map_err(|e| e.to_string())?);
    let t0 = Instant::now();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let got: usize = (first..first + n).into_par_iter().map(|i| {
        let u = [halton(i, 2), halton(i, 3), halton(i, 5), halton(i, 7), halton(i, 11)];
        let c: [f64; 5] = std::array::from_fn(|k| lo[k] + u[k] * (hi[k] - lo[k]));
        let s = exit_state(&c);
        let k = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if k % 50 == 0 { eprintln!("{k}/{n} ({:.0} s)", t0.elapsed().as_secs_f64()); }
        if walls.clearance([s[0], s[1], s[2]], [s[0], s[1], s[2]]).0 < MARGIN { return 0; }
        let Some(p) = merge(&walls, &r, &s, MERGE_T) else { return 0 };
        let rec = Sample { c, n: p.u.len(), t: MERGE_T, value: (p.u.len() + total - MERGE_T) as f64 + TAU };
        let mut f = out.lock().unwrap();
        writeln!(f, "{}", serde_json::to_string(&rec).unwrap()).unwrap();
        1
    }).sum();
    println!("{got} of {n} solved in {:.0} s, reference {total}", t0.elapsed().as_secs_f64());
    Ok(())
}

/// `runs/dome/table.json` as the exit gate's table, one `tau`.
fn load_table(dir: &Path) -> Result<Arc<ValueTable>, String> {
    #[derive(Deserialize)]
    struct J {
        nodes: Vec<Vec<f64>>,
        values: Vec<f64>,
    }
    let j: J = serde_json::from_str(&std::fs::read_to_string(dir.join("runs/dome/table.json")).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let n = j.nodes;
    Ok(Arc::new(ValueTable {
        gate: exit_gate(),
        nodes: [n[0].clone(), n[1].clone(), n[2].clone(), n[3].clone(), n[4].clone(), vec![TAU]],
        values: j.values,
    }))
}

/// The room's columns under the cone, each with the highest feet that fit at its center.
fn roof_spots(w: &World, walls: &Walls) -> Vec<[f64; 3]> {
    let cells = flood(w);
    let mut top: std::collections::BTreeMap<(i32, i32), i32> = Default::default();
    for c in &cells {
        let e = top.entry((c[0], c[2])).or_insert(c[1]);
        *e = (*e).max(c[1]);
    }
    let mut out = Vec::new();
    for ((x, z), t) in top {
        if t < CONE_FROM {
            continue;
        }
        let (px, pz) = (x as f64 + 0.5, z as f64 + 0.5);
        // from the block above down: a stair or slab overhead leaves part of a block
        let mut y = (t + 2) as f64;
        while y > t as f64 - 1.0 {
            let p = [px, y, pz];
            if walls.clearance(p, p).0 >= ROOF_CLEAR && flood_has(&cells, [x, y.floor() as i32, z]) {
                out.push(p);
                break;
            }
            y -= 0.05;
        }
    }
    out
}

fn flood_has(cells: &[[i32; 3]], c: [i32; 3]) -> bool {
    cells.contains(&c)
}

/// A funnel: from where it starts, maybe by a point to pull out at, through the door.
fn funnels(p: [f64; 3]) -> Vec<Vec<[f64; 3]>> {
    let mut out = Vec::new();
    let line = |a: [f64; 3], b: [f64; 3]| -> Vec<[f64; 3]> {
        let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        let n = (d.ceil() as usize).max(1);
        (0..n).map(|i| std::array::from_fn(|k| a[k] + (b[k] - a[k]) * i as f64 / n as f64)).collect()
    };
    for door_y in [206.5, 203.0] {
        let door = [DOOR_X, door_y, 209.0];
        let past = [DOOR_X, door_y - 2.0, 198.0];
        for via in [None, Some([DOOR_X, 232.0, 221.0]), Some([DOOR_X, 222.0, 216.0])] {
            let mut path = Vec::new();
            match via {
                None => path.extend(line(p, door)),
                Some(q) => {
                    path.extend(line(p, q));
                    path.extend(line(q, door));
                }
            }
            path.extend(line(door, past));
            path.push(past);
            out.push(path);
        }
    }
    out
}

#[derive(Serialize, Deserialize, Clone)]
struct Start {
    x: X,
    /// What the exit gate's table reads where this crosses: timed ticks.
    value: f64,
    ticks: usize,
    exit: X,
    u: Vec<U>,
}

/// The best way from `x0` through the door, against the table, time before it free.
fn from_start(walls: &Walls, table: &Arc<ValueTable>, x0: X, seeds: &[Vec<U>]) -> Option<Start> {
    let mut best: Option<(f64, Plan)> = None;
    for seed in seeds {
        let pb = Problem {
            kernel: Generic::default(),
            walls,
            x0,
            goal: Goal::Table { table: table.clone(), cap: seed.len() + 60, time_weight: 0.0 },
            opts: opts(),
        };
        let sol = pb.solve(seed);
        if !solved(&pb, &sol.plan) || !sol.plan.x.iter().all(|x| walls.covers([x[0], x[1], x[2]])) {
            continue;
        }
        let v = pb.value(&sol.plan.x);
        if best.as_ref().is_none_or(|b| v < b.0) {
            best = Some((v, sol.plan));
        }
    }
    let (value, plan) = best?;
    Some(Start { x: x0, value, ticks: plan.u.len(), exit: *plan.x.last().unwrap(), u: plan.u })
}

/// Column-wise neighbor seeding for `Start`s: try each one's controls from the rest of `spots`.
fn neighbor_seeds(found: &[Start], x0: &X, top: usize) -> Vec<Vec<U>> {
    let mut near: Vec<&Start> = found.iter().collect();
    near.sort_by(|a, b| {
        let d = |s: &Start| (s.x[0] - x0[0]).powi(2) + (s.x[2] - x0[2]).powi(2) + (s.x[1] - x0[1]).powi(2);
        d(a).total_cmp(&d(b))
    });
    near.truncate(NEAR);
    near.sort_by(|a, b| a.value.total_cmp(&b.value));
    near.into_iter().take(top).map(|s| s.u.clone()).collect()
}

fn dome_walls(w: &World) -> Walls {
    Walls::region(w, [-44, 190, 192], [-10, 303, 236], 1.0)
}

fn funnel_seeds(walls: &Walls, x0: &X) -> Vec<Vec<U>> {
    funnels([x0[0], x0[1], x0[2]])
        .iter()
        .flat_map(|path| {
            [(10, 0.3), (16, 1.0)].map(|(ahead, cross)| guided(walls, x0, path, ahead, cross, HEAD_CAP))
        })
        .collect()
}

/// `starts`: every spot under the roof against the table.
fn starts(dir: &Path, w: &World) -> Result<(), String> {
    let table = load_table(dir)?;
    let walls = dome_walls(w);
    let spots = roof_spots(w, &walls);
    println!("{} spots under the roof", spots.len());
    let t0 = Instant::now();
    // why a nudge toward the door? a velocity of exactly nothing has no heading to differentiate
    let rest = |p: &[f64; 3]| -> X { [p[0], p[1], p[2], 0.0, 0.0, -REST] };
    let mut found: Vec<Start> = spots
        .par_iter()
        .filter_map(|p| {
            let x0 = rest(p);
            let mut seeds = pull_seeds(&walls, &table, &x0);
            seeds.extend(funnel_seeds(&walls, &x0).into_iter().take(2));
            from_start(&walls, &table, x0, &seeds)
        })
        .collect();
    println!("funnels: {} solved, {:.0} s", found.len(), t0.elapsed().as_secs_f64());
    for sweep in 0..SWEEPS {
        let before = found.clone();
        let new: Vec<Start> = spots
            .par_iter()
            .filter_map(|p| {
                let x0 = rest(p);
                let mut seeds = neighbor_seeds(&before, &x0, 3);
                seeds.extend(before.iter().find(|s| s.x[0] == x0[0] && s.x[2] == x0[2]).map(|s| s.u.clone()));
                let got = from_start(&walls, &table, x0, &seeds);
                let had = before.iter().find(|s| s.x[0] == x0[0] && s.x[2] == x0[2]);
                match (got, had) {
                    (Some(g), Some(h)) => Some(if g.value < h.value { g } else { h.clone() }),
                    (g, h) => g.or(h.cloned()),
                }
            })
            .collect();
        found = new;
        let best = found.iter().map(|s| s.value).fold(f64::INFINITY, f64::min);
        println!("sweep {sweep}: {} solved, best {best:.2}, {:.0} s", found.len(), t0.elapsed().as_secs_f64());
    }
    found.sort_by(|a, b| a.value.total_cmp(&b.value));
    print_starts(&found, 40);
    std::fs::write(dir.join("runs/dome/starts.json"), serde_json::to_string(&found).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

fn print_starts(found: &[Start], n: usize) {
    println!("value   start x      y      z   | v0 x     y     z  | ticks | exit x      y      vx    vy    vz");
    for s in found.iter().take(n) {
        let e = s.exit;
        println!(
            "{:7.2} {:6.1} {:6.1} {:6.1} | {:5.2} {:5.2} {:5.2} | {:3} | {:6.2} {:6.2} {:5.2} {:5.2} {:5.2}",
            s.value, s.x[0], s.x[1], s.x[2], s.x[3], s.x[4], s.x[5], s.ticks, e[0], e[1], e[3], e[4], e[5]
        );
    }
}

/// What the table reads where a rollout first crosses the exit plane, and the controls to there.
fn exit_read(table: &ValueTable, prev: &X, x: &X) -> Option<f64> {
    if prev[2] < EXIT_Z || x[2] >= EXIT_Z || x[5] >= 0.0 || !table.gate.in_window(x) {
        return None;
    }
    let tau = (EXIT_Z - x[2]) / -x[5];
    Some(table.read(&table.gate.coords(x, tau)))
}

/// Funneling the velocity: controls that glide away from the door, dive, and pull out at it, swept
/// over when each starts and how hard, flown and scored on the table. The best few clear ones.
/// why not a path to follow? a follower creeps along it, reaching the door at 1.2 b/t from a
/// 95 block drop, and a solve from there stays near that (academy's apex, 2026-11-05).
fn pull_seeds(walls: &Walls, table: &ValueTable, x0: &X) -> Vec<Vec<U>> {
    let k = Generic::default();
    let mut scored: Vec<(f64, Vec<U>)> = Vec::new();
    let aim = |x: &X, to: [f64; 2]| -> f64 {
        // the lead that looks at `to`, in yaw's convention (look = (-sin yaw, cos yaw))
        let want = (-(to[0] - x[0])).atan2(to[1] - x[2]);
        let have = (-x[3]).atan2(x[5]);
        let d = want - have;
        (d + PI).rem_euclid(2.0 * PI) - PI
    };
    for away in [0usize, 6, 12, 18, 26, 34] {
        for away_pitch in [15.0f64, 40.0] {
            if away == 0 && away_pitch != 15.0 {
                continue;
            }
            for dive in (10..=70).step_by(3) {
                for pull in [-10.0f64, -4.0, 0.0, 4.0, 8.0, 14.0, 22.0, 32.0] {
                    let mut x = *x0;
                    let mut u = Vec::new();
                    let mut read = None;
                    let mut clear = true;
                    for t in 0..HEAD_CAP {
                        let c: U = if t < away {
                            [aim(&x, [DOOR_X, FAR_Z]), away_pitch.to_radians()]
                        } else if t < away + dive {
                            [aim(&x, [DOOR_X, 209.0]), 85f64.to_radians()]
                        } else {
                            [aim(&x, [DOOR_X, 205.0]), pull.to_radians()]
                        };
                        let (n, corners) = step(&k, &x, c);
                        if corners.windows(2).any(|l| walls.clearance(l[0], l[1]).0 < MARGIN) {
                            clear = false;
                            break;
                        }
                        u.push(c);
                        if let Some(v) = exit_read(table, &x, &n) {
                            read = Some(v);
                            break;
                        }
                        x = n;
                    }
                    if let (true, Some(v)) = (clear, read) {
                        scored.push((v, u));
                    }
                }
            }
        }
    }
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.truncate(PULL_SEEDS);
    scored.into_iter().map(|s| s.1).collect()
}

/// The best starts as flights for `optimize --hybrid --open-top`: the way down flown under port
/// from the start, the merge into the reference, and the reference's states after it.
/// `flights N`: the best `N` spots at least [`APART`] apart.
fn flights(dir: &Path, course: &Course, w: &World, n: usize) -> Result<(), String> {
    let r = reference(dir, course)?;
    let found: Vec<Start> = serde_json::from_str(&std::fs::read_to_string(dir.join("runs/dome/starts.json")).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut chosen: Vec<&Start> = Vec::new();
    for s in &found {
        let far = |c: &&Start| ((c.x[0] - s.x[0]).powi(2) + (c.x[1] - s.x[1]).powi(2) + (c.x[2] - s.x[2]).powi(2)).sqrt() >= APART;
        if chosen.iter().all(|c| far(c)) {
            chosen.push(s);
            if chosen.len() == n {
                break;
            }
        }
    }
    let mut pts: Vec<[f64; 3]> = r.rec.rows.iter().map(|x| x.state.position).collect();
    pts.push([-26.5, 301.0, 220.0]);
    let walls = Walls::around(w, &pts, 8.0, 1.0);
    let proto = Trajectory::read(&dir.join("runs/relax/fullrange2.traj.jsonl"))?;
    let keys = r.rec.rows[r.exit].input.keys.replace('J', "");
    let out = dir.join("runs/hybrid");
    let total = r.x.len() - 1;
    for (k, st) in chosen.iter().enumerate() {
        // the way down, under port
        let mut initial = proto.initial.clone();
        initial.position = [st.x[0], st.x[1], st.x[2]];
        initial.delta_movement = [st.x[3], st.x[4], st.x[5]];
        let plan = Problem { kernel: Generic::default(), walls: &walls, x0: st.x, goal: Goal::Plane { normal: [0.0, 0.0, -1.0], at: -EXIT_Z, cap: st.u.len() + 5 }, opts: opts() };
        let xs = plan.rollout(&st.u);
        let mut yaw = 180.0;
        let mut inputs = Vec::new();
        for (t, u) in st.u.iter().enumerate() {
            let (input, y) = input_at(&xs[t], u, yaw);
            yaw = y;
            inputs.push(InputRow { yaw: input.yaw as f64, pitch: input.pitch as f64, keys: keys.clone() });
        }
        let mut h = r.rec.header.clone();
        h.source = format!("dome {k}: from {:.1} {:.1} {:.1}", st.x[0], st.x[1], st.x[2]);
        let (head, stopped) = fly(&initial, &inputs, &[], w, h.clone());
        if let Some((t, e)) = stopped {
            println!("dome_{k}: port stopped at {t}: {e:?}");
            continue;
        }
        let exit = (1..=head.rows.len()).find(|&t| state_at(&head, t).position[2] < EXIT_Z).ok_or("the way down has no exit")?;
        let s = flight(state_at(&head, exit));
        let Some(join) = merge(&walls, &r, &s, MERGE_T) else {
            println!("dome_{k}: no merge from {s:?}");
            continue;
        };
        let mut rows = head.rows[..exit].to_vec();
        let proto_row = r.rec.rows[r.exit].clone();
        let mut near = rows.last().map_or(yaw, |x| x.input.yaw);
        for (t, u) in join.u.iter().enumerate() {
            let (input, y) = input_at(&join.x[t], u, near);
            near = y;
            let mut row = proto_row.clone();
            row.t = rows.len() + 1;
            row.input.yaw = input.yaw as f64;
            row.input.pitch = input.pitch as f64;
            let e = join.x[t + 1];
            row.state.position = [e[0], e[1], e[2]];
            row.state.delta_movement = [e[3], e[4], e[5]];
            rows.push(row);
        }
        for t in r.exit + MERGE_T..r.rec.rows.len() {
            let mut row = r.rec.rows[t].clone();
            row.t = rows.len() + 1;
            rows.push(row);
        }
        let traj = Trajectory { header: h, initial, arrivals: Vec::new(), rows };
        traj.write(&out.join(format!("dome_{k}.traj.jsonl")))?;
        println!(
            "dome_{k}: from {:.1} {:.1} {:.1}, table {:.2}, exit y {:.2} v {:.2} {:.2} {:.2}, merges in {} for {} timed",
            st.x[0], st.x[1], st.x[2], st.value, s[1], s[3], s[4], s[5], join.u.len(), join.u.len() + total - MERGE_T
        );
    }
    Ok(())
}

/// The results in the room: the roof colored by what each spot under it is worth, and the best
/// ways down, cut open from the side and the front.
fn results(dir: &Path, w: &World) -> Result<(), String> {
    let found: Vec<Start> = serde_json::from_str(&std::fs::read_to_string(dir.join("runs/dome/starts.json")).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let lo = found.iter().map(|s| s.value).fold(f64::INFINITY, f64::min);
    let hi = lo + VIEW_RANGE;
    let by_column: std::collections::HashMap<(i32, i32), f64> =
        found.iter().map(|s| ((s.x[0].floor() as i32, s.x[2].floor() as i32), s.value)).collect();
    let ramp = Ramp::magnitude();
    let shade = |v: f64| ramp.at(1.0 - (v - lo) / (hi - lo));
    let scene = Scene::new(w)?;
    let out = dir.join("runs/dome");
    let walls = dome_walls(w);
    let flown: Vec<(usize, Vec<[f64; 3]>)> = found
        .iter()
        .take(TOP_DRAWN)
        .enumerate()
        .map(|(i, s)| {
            let pb = Problem { kernel: Generic::default(), walls: &walls, x0: s.x, goal: Goal::Plane { normal: [0.0, 0.0, -1.0], at: -EXIT_Z, cap: s.u.len() + 5 }, opts: opts() };
            (i, pb.rollout(&s.u).iter().map(|x| [x[0], x[1], x[2]]).collect())
        })
        .collect();
    let bounds = vec![[-40.0, 194.0, 196.0], [-14.0, 302.0, 233.0]];
    let draw = |f: &mut view::Frame| {
        for (i, path) in flown.iter().rev() {
            let c = if *i == 0 { WHITE } else { YELLOW };
            f.polyline(path, Stroke::new(c, if *i == 0 { 3.0 } else { 1.5 }).occluded(0.7));
            f.point(path[0], Marker::circle(c, if *i == 0 { 9.0 } else { 5.0 }));
        }
    };
    {
        let mut f = scene.frame(&Camera::top(&bounds), 1100, 1000);
        for ((x, z), v) in &by_column {
            let (x, z) = (*x as f64, *z as f64);
            let q = |dx: f64, dz: f64| [x + dx, MAP_Y, z + dz];
            f.fill_quad(q(0.0, 0.0), q(1.0, 0.0), q(1.0, 1.0), q(0.0, 1.0), shade(*v));
        }
        tools::rulers(&mut f);
        tools::compass(&mut f, None);
        tools::title(&mut f, "roof: timed ticks a start under it reads");
        tools::colorbar(&mut f, &Ramp(ramp.0.iter().rev().copied().collect()), lo, hi, "ticks");
        f.finish().save(out.join("roof_value.png"))?;
    }
    for (name, cam, style) in [
        ("best_side", Camera::side(&bounds, 90.0), WorldStyle::new().slice(0, -60.0, DOOR_X)),
        ("best_front", Camera::side(&bounds, 180.0), WorldStyle::new().slice(2, 180.0, 219.5)),
    ] {
        let mut f = scene.frame(&cam, 1000, 1300);
        f.style(style);
        draw(&mut f);
        tools::rulers(&mut f);
        tools::compass(&mut f, None);
        tools::title(&mut f, name);
        f.finish().save(out.join(format!("{name}.png")))?;
    }
    Ok(())
}

/// A check on `starts` that owes nothing to a solve or a seed: the cross-entropy method over a
/// rotation every [`KNOT`] ticks, flown through the walls, scored on the table. Slow to converge,
/// but it can't share a solve's local minimum. `global`: some starts, with and without a velocity.
fn global(dir: &Path, w: &World) -> Result<(), String> {
    let table = load_table(dir)?;
    let walls = dome_walls(w);
    let k = Generic::default();
    let spots: [([f64; 3], [f64; 3]); 8] = [
        ([-27.5, 298.3, 221.5], [0.0, 0.0, -REST]),
        ([-26.5, 300.0, 220.5], [0.0, 0.0, -REST]),
        ([-31.5, 292.3, 221.5], [0.0, 0.0, -REST]),
        ([-26.5, 288.0, 226.5], [0.0, 0.0, -REST]),
        ([-26.5, 300.0, 220.5], [0.0, 0.0, 0.5]),
        ([-26.5, 300.0, 220.5], [0.0, 0.0, -0.5]),
        ([-26.5, 300.0, 220.5], [0.0, 0.0, 1.0]),
        ([-26.5, 300.0, 220.5], [0.0, 0.0, -1.0]),
    ];
    // absolute yaw and pitch per knot, radians: flown as a lead off each tick's heading
    let fly_knots = |x0: &X, g: &[f64]| -> (f64, Vec<U>, X) {
        let mut x = *x0;
        let mut u = Vec::new();
        for t in 0..KNOTS * KNOT {
            let i = t / KNOT;
            let (yaw, pitch) = (g[2 * i], g[2 * i + 1].clamp(-1.57, 1.57));
            let have = (-x[3]).atan2(x[5]);
            let c: U = [(yaw - have + PI).rem_euclid(2.0 * PI) - PI, pitch];
            let (n, corners) = step(&k, &x, c);
            if corners.windows(2).any(|l| walls.clearance(l[0], l[1]).0 < MARGIN) {
                // how far it got toward the door, so a crash still ranks
                let d = ((n[0] - DOOR_X).powi(2) + (n[1] - 203.0).powi(2) + (n[2] - 205.0).powi(2)).sqrt();
                return (CRASH + d, u, x);
            }
            u.push(c);
            if let Some(v) = exit_read(&table, &x, &n) {
                return (v, u, n);
            }
            x = n;
        }
        (CRASH * 2.0, u, x)
    };
    let lines: Vec<String> = spots
        .par_iter()
        .enumerate()
        .flat_map(|(si, (p, v))| {
            // those heights are rounded, and a hair too high is inside the roof
            let x0: X = [p[0], p[1] - 0.1, p[2], v[0], v[1], v[2]];
            let fly_knots = &fly_knots;
            let table = &table;
            let walls = &walls;
            (0..RESTARTS).into_par_iter().map(move |r| {
                let mut rng = 0x9E3779B97F4A7C15u64 ^ ((si as u64 + 1) << 32) ^ (r as u64 + 1);
                let mut uni = move || {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    (rng >> 11) as f64 / (1u64 << 53) as f64
                };
                // heading for the door and diving, widely
                let dim = 2 * KNOTS;
                let mut mean: Vec<f64> = (0..dim).map(|i| if i % 2 == 0 { PI } else { 0.8 }).collect();
                let mut sd: Vec<f64> = (0..dim).map(|i| if i % 2 == 0 { 1.5 } else { 0.9 }).collect();
                let mut best = (f64::INFINITY, Vec::new(), [0.0; 6]);
                for _ in 0..CEM_ROUNDS {
                    let mut pop: Vec<(f64, Vec<f64>)> = (0..CEM_POP)
                        .map(|_| {
                            let g: Vec<f64> = (0..dim)
                                .map(|i| {
                                    let z = (-2.0 * uni().max(1e-12).ln()).sqrt() * (2.0 * PI * uni()).cos();
                                    mean[i] + sd[i] * z
                                })
                                .collect();
                            let (v, u, e) = fly_knots(&x0, &g);
                            if v < best.0 {
                                best = (v, u, e);
                            }
                            (v, g)
                        })
                        .collect();
                    pop.sort_by(|a, b| a.0.total_cmp(&b.0));
                    pop.truncate(CEM_ELITE);
                    for i in 0..dim {
                        let m = pop.iter().map(|e| e.1[i]).sum::<f64>() / CEM_ELITE as f64;
                        let var = pop.iter().map(|e| (e.1[i] - m).powi(2)).sum::<f64>() / CEM_ELITE as f64;
                        mean[i] = m;
                        sd[i] = var.sqrt() + 0.003;
                    }
                }
                let (v, u, e) = best;
                // and a solve from what it found
                if u.is_empty() {
                    return format!("from {p:?}: nothing got off the start");
                }
                let polished = from_start(walls, table, x0, std::slice::from_ref(&u));
                let (pv, pe) = polished.map_or((f64::NAN, [f64::NAN; 6]), |s| (s.value, s.exit));
                format!(
                    "from {:.1} {:.1} {:.1} v0 {:.1} {:.1}: search {v:.2} exit y{:.1} v {:.2} {:.2} {:.2}; solved {pv:.2} exit y{:.1} v {:.2} {:.2} {:.2}",
                    p[0], p[1], p[2], v0(x0), x0[5], e[1], e[3], e[4], e[5], pe[1], pe[3], pe[4], pe[5]
                )
            })
        })
        .collect();
    for l in lines {
        println!("{l}");
    }
    Ok(())
}

fn v0(x: X) -> f64 {
    x[3]
}

/// What holds the exit where it is: [`global`]'s search from the apex with and without the walls,
/// for the table and for raw speeds. Best exit per case.
fn frontier(dir: &Path, w: &World) -> Result<(), String> {
    let table = load_table(dir)?;
    let room = dome_walls(w);
    let air = World::air(world::BlockPos { x: -44, y: 190, z: 150 }, world::BlockPos { x: -10, y: 303, z: 236 });
    let open = Walls::region(&air, [-44, 190, 192], [-10, 303, 236], 1.0);
    let k = Generic::default();
    let score = |obj: usize, x: &X, n: &X| -> Option<f64> {
        let v = exit_read(&table, x, n)?;
        Some(match obj {
            0 => v,
            1 => n[5],
            _ => -(n[3] * n[3] + n[4] * n[4] + n[5] * n[5]).sqrt(),
        })
    };
    let x0: X = [-26.5, 299.9, 220.5, 0.0, 0.0, -REST];
    let cases: Vec<(&str, &Walls, usize)> = vec![
        ("room, table", &room, 0), ("room, forward speed", &room, 1), ("room, speed", &room, 2),
        ("no walls, table", &open, 0), ("no walls, forward speed", &open, 1), ("no walls, speed", &open, 2),
    ];
    for (name, walls, obj) in cases {
        let runs: Vec<(f64, X, f64)> = (0..RESTARTS * 4).into_par_iter().map(|r| {
            let mut rng = 0x9E3779B97F4A7C15u64 ^ ((obj as u64 + 1) << 40) ^ (r as u64 + 1);
            let mut uni = move || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (rng >> 11) as f64 / (1u64 << 53) as f64
            };
            let dim = 2 * KNOTS;
            let mut mean: Vec<f64> = (0..dim).map(|i| if i % 2 == 0 { PI } else { 0.8 }).collect();
            let mut sd: Vec<f64> = (0..dim).map(|i| if i % 2 == 0 { 1.5 } else { 0.9 }).collect();
            let mut best = (f64::INFINITY, x0);
            for _ in 0..CEM_ROUNDS {
                let mut pop: Vec<(f64, Vec<f64>)> = (0..CEM_POP).map(|_| {
                    let g: Vec<f64> = (0..dim).map(|i| {
                        let z = (-2.0 * uni().max(1e-12).ln()).sqrt() * (2.0 * PI * uni()).cos();
                        mean[i] + sd[i] * z
                    }).collect();
                    let mut x = x0;
                    let mut val = CRASH * 2.0;
                    for t in 0..KNOTS * KNOT {
                        let i = t / KNOT;
                        let have = (-x[3]).atan2(x[5]);
                        let c: U = [(g[2 * i] - have + PI).rem_euclid(2.0 * PI) - PI, g[2 * i + 1].clamp(-1.57, 1.57)];
                        let (n, corners) = step(&k, &x, c);
                        if corners.windows(2).any(|l| walls.clearance(l[0], l[1]).0 < MARGIN) {
                            let d = ((n[0] - DOOR_X).powi(2) + (n[1] - 203.0).powi(2) + (n[2] - 205.0).powi(2)).sqrt();
                            val = CRASH + d;
                            break;
                        }
                        if let Some(v) = score(obj, &x, &n) {
                            val = v;
                            if v < best.0 { best = (v, n); }
                            break;
                        }
                        x = n;
                    }
                    (val, g)
                }).collect();
                pop.sort_by(|a, b| a.0.total_cmp(&b.0));
                pop.truncate(CEM_ELITE);
                for i in 0..dim {
                    let m = pop.iter().map(|e| e.1[i]).sum::<f64>() / CEM_ELITE as f64;
                    let var = pop.iter().map(|e| (e.1[i] - m).powi(2)).sum::<f64>() / CEM_ELITE as f64;
                    mean[i] = m;
                    sd[i] = var.sqrt() + 0.003;
                }
            }
            let e = best.1;
            let tau = (EXIT_Z - e[2]) / -e[5];
            (best.0, e, table.read(&table.gate.coords(&e, tau)))
        }).collect();
        let b = runs.iter().min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
        let e = b.1;
        println!("{name:26} exit x{:.1} y{:.1} v {:.2} {:.2} {:.2} speed {:.2}, table reads {:.1}", e[0], e[1], e[3], e[4], e[5], (e[3]*e[3]+e[4]*e[4]+e[5]*e[5]).sqrt(), b.2);
    }
    Ok(())
}

const SLAB_SETTLE: usize = 6;

/// One way off the slab: where it stands, its heading, ticks of run-up, the air tick the sprint comes
/// on (0: a sprint jump, none: never), and the air tick it deploys on.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Off {
    spot: [f64; 3],
    yaw: f64,
    run: usize,
    coast: Option<usize>,
    deploy: usize,
    #[serde(default, flatten)]
    more: OffMore,
    /// Just after the deploy, what the table reads at the door, ticks and state there.
    x: X,
    value: f64,
    ticks: usize,
    exit: X,
    u: Vec<U>,
}

/// The rest of a way off the slab: air ticks of W held after the jump, then of S; the sprint's and the
/// deploy tick's yaw from `yaw`, and the deploy tick's pitch.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
#[serde(default)]
struct OffMore {
    hold: usize,
    brake: usize,
    air_turn: f64,
    dep_turn: f64,
    dep_pitch: f64,
    /// How the `brake` ticks push south: 0, S; 1, W + sprint, turned around; 2, the same on a 45 degree strafe.
    back: u8,
    /// The sprint north on a 45 degree strafe.
    fwd45: bool,
    /// Touches a wall in the air, sideways, and only then: why apart? to see if the corner's stop costs.
    contact: bool,
}

fn off_inputs(yaw: f64, run: usize, coast: Option<usize>, deploy: usize, m: OffMore) -> Vec<InputRow> {
    let row = |k: &str| InputRow { yaw, pitch: 0.0, keys: k.to_string() };
    let mut v = vec![row("-"); SLAB_SETTLE];
    v.extend(vec![row("WR"); run]);
    v.push(row(if coast == Some(0) || run > 0 || m.hold > 0 { "WRJ" } else { "J" }));
    for a in 1..deploy {
        let (y, k) = if coast.is_some_and(|c| a >= c) {
            if m.fwd45 { (yaw + m.air_turn + 45.0, "WAR") } else { (yaw + m.air_turn, "WR") }
        } else if a <= m.hold {
            (yaw, "WR")
        } else if a <= m.hold + m.brake {
            match m.back {
                0 => (yaw, "S"),
                1 => (yaw + 180.0, "WR"),
                _ => (yaw + 225.0, "WAR"),
            }
        } else {
            (yaw, "-")
        };
        v.push(InputRow { yaw: y, pitch: 0.0, keys: k.to_string() });
    }
    v.push(InputRow { yaw: yaw + m.dep_turn, pitch: m.dep_pitch, keys: if coast.is_some_and(|c| deploy >= c) { "WRJ" } else { "J" }.to_string() });
    v
}

/// The state just after the deploy, flown under port from standing on `spot`: None if it hits anything
/// or leaves the slab early.
fn off_state(proto: &Trajectory, w: &World, spot: [f64; 3], yaw: f64, run: usize, coast: Option<usize>, deploy: usize, more: OffMore) -> Option<X> {
    let mut initial = proto.initial.clone();
    initial.position = spot;
    initial.delta_movement = [0.0, 0.0, 0.0];
    initial.y_rot = yaw;
    initial.x_rot = 0.0;
    initial.on_ground = false;
    initial.main_supporting_block_pos = None;
    let inputs = off_inputs(yaw, run, coast, deploy, more);
    let (t, stopped) = fly(&initial, &inputs, &[], w, proto.header.clone());
    if stopped.is_some() {
        return None;
    }
    let jump = SLAB_SETTLE + run;
    if !t.rows[SLAB_SETTLE - 1..jump].iter().all(|r| r.state.on_ground) {
        return None;
    }
    let air = &t.rows[jump..];
    if air.iter().any(|r| r.state.on_ground || r.state.vertical_collision) || air.iter().any(|r| r.state.horizontal_collision) != more.contact {
        return None;
    }
    let last = &air.last()?.state;
    last.fall_flying.then(|| flight(last))
}

/// The dives and pull-outs from `x0` that reach the door, best first, as [`pull_seeds`] with no glide away.
fn dive_seeds(walls: &Walls, table: &ValueTable, x0: &X, keep: usize) -> Vec<(f64, Vec<U>)> {
    let k = Generic::default();
    let aim = |x: &X, to: [f64; 2]| -> f64 {
        let want = (-(to[0] - x[0])).atan2(to[1] - x[2]);
        let have = (-x[3]).atan2(x[5]);
        (want - have + PI).rem_euclid(2.0 * PI) - PI
    };
    let mut scored: Vec<(f64, Vec<U>)> = Vec::new();
    for dive in (0..=64).step_by(4) {
        for pull in [-10.0f64, -4.0, 0.0, 4.0, 8.0, 14.0, 22.0, 32.0] {
            let (mut x, mut u, mut read) = (*x0, Vec::new(), None);
            for t in 0..HEAD_CAP {
                let c: U = if t < dive { [aim(&x, [DOOR_X, 209.0]), 85f64.to_radians()] } else { [aim(&x, [DOOR_X, 205.0]), pull.to_radians()] };
                let (n, corners) = step(&k, &x, c);
                if corners.windows(2).any(|l| walls.clearance(l[0], l[1]).0 < MARGIN) {
                    break;
                }
                u.push(c);
                if let Some(v) = exit_read(table, &x, &n) {
                    read = Some(v);
                    break;
                }
                x = n;
            }
            if let Some(v) = read {
                scored.push((v, u));
            }
        }
    }
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.truncate(keep);
    scored
}

/// `slab`: the ways off the slab against the table, and the current start's for comparison.
fn slab(dir: &Path, w: &World) -> Result<(), String> {
    let table = load_table(dir)?;
    let walls = dome_walls(w);
    let rec = Trajectory::read(&dir.join("runs/tas_gen1_34/academy.traj.jsonl"))?;
    let t0 = Instant::now();

    // the current start, on the same footing: its own exit, and a solve from the top of its flight
    let exit = (1..rec.rows.len()).find(|&t| state_at(&rec, t).position[2] < EXIT_Z).ok_or("no exit")?;
    let (a, b) = (flight(state_at(&rec, exit - 1)), flight(state_at(&rec, exit)));
    println!("current TAS: exit tick {exit}, y {:.2} v {:.3} {:.3} {:.3}, table reads {:.2}", b[1], b[3], b[4], b[5], exit_read(&table, &a, &b).unwrap_or(f64::NAN));
    let apex = (1..exit).max_by(|&i, &j| state_at(&rec, i).position[1].total_cmp(&state_at(&rec, j).position[1])).unwrap();
    let x_apex = flight(state_at(&rec, apex));
    let own: Vec<U> = (apex..exit).map(|t| { let i = &rec.rows[t].input; control(lead_of(&flight(state_at(&rec, t)), i.yaw), i.pitch) }).collect();
    let mut seeds = vec![own];
    seeds.extend(pull_seeds(&walls, &table, &x_apex));
    seeds.extend(dive_seeds(&walls, &table, &x_apex, 3).into_iter().map(|s| s.1));
    match from_start(&walls, &table, x_apex, &seeds) {
        Some(s) => println!("current TAS re-solved from its apex (tick {apex}, y {:.2}): table {:.2}, exit y {:.2} v {:.3} {:.3} {:.3}", x_apex[1], s.value, s.exit[1], s.exit[3], s.exit[4], s.exit[5]),
        None => println!("current TAS: no solve from its apex"),
    }
    let dome_best: Option<f64> = std::fs::read_to_string(dir.join("runs/dome/starts.json")).ok().and_then(|t| serde_json::from_str::<Vec<Start>>(&t).ok()).map(|f| f.iter().map(|s| s.value).fold(f64::INFINITY, f64::min));
    println!("under the roof, at rest (`starts`): best table {dome_best:?}");

    // a stage of rollouts scored by a dive family, then solves from the best
    // around the optimum of a wider grid: run 0..=10, sprint on from 0 to 44 or never, deploy 3 to 78
    let mut grid = Vec::new();
    for x in [-25.8, -25.5, -25.2] {
        for z in [227.1, 227.5, 227.9, 228.3, 228.68] {
            for yaw in [168.0, 170.0, 172.0, 174.0, 176.0, 178.0, 180.0, 182.0] {
                for run in 0..=5usize {
                    for coast in [None, Some(10), Some(14), Some(16), Some(18), Some(20), Some(22), Some(24), Some(26), Some(28), Some(30), Some(32), Some(34), Some(36), Some(40)] {
                        grid.push(([x, 283.7, z], yaw, run, coast));
                    }
                }
            }
        }
    }
    let deploys: Vec<usize> = (40..=57).collect();
    let proto = &rec;
    let scored: Vec<Off> = grid
        .par_iter()
        .flat_map_iter(|&(spot, yaw, run, coast)| {
            let table = &table;
            let walls = &walls;
            deploys.iter().filter_map(move |&deploy| {
                let more = OffMore::default();
                let x = off_state(proto, w, spot, yaw, run, coast, deploy, more)?;
                let (value, u) = dive_seeds(walls, table, &x, 1).into_iter().next()?;
                Some(Off { spot, yaw, run, coast, deploy, more, x, value, ticks: u.len(), exit: x, u })
            })
        })
        .collect();
    println!("{} rollouts reach the door out of {} tries, {:.0} s", scored.len(), grid.len() * deploys.len(), t0.elapsed().as_secs_f64());
    let mut scored = scored;
    scored.sort_by(|a, b| a.value.total_cmp(&b.value));
    let polish = 1500.min(scored.len());
    let mut solved: Vec<Off> = scored[..polish]
        .par_iter()
        .filter_map(|o| {
            let mut seeds: Vec<Vec<U>> = vec![o.u.clone()];
            seeds.extend(dive_seeds(&walls, &table, &o.x, 3).into_iter().map(|s| s.1));
            let s = from_start(&walls, &table, o.x, &seeds)?;
            Some(Off { value: s.value, ticks: s.ticks, exit: s.exit, u: s.u, ..o.clone() })
        })
        .collect();
    solved.sort_by(|a, b| a.value.total_cmp(&b.value));
    println!("{} solved, {:.0} s", solved.len(), t0.elapsed().as_secs_f64());
    println!("table  spot x     z     yaw run coast deploy | deploy y  v y    v z  | ticks | exit y   vx    vy    vz");
    for o in solved.iter().take(30) {
        let e = o.exit;
        println!(
            "{:7.2} {:6.1} {:6.2} {:4.0} {:3} {:>5} {:3} | {:6.2} {:5.2} {:5.2} | {:3} | {:6.2} {:5.2} {:5.2} {:5.2}",
            o.value, o.spot[0], o.spot[2], o.yaw, o.run, o.coast.map_or("-".to_string(), |c| c.to_string()), o.deploy, o.x[1], o.x[4], o.x[5], o.ticks, e[1], e[3], e[4], e[5]
        );
    }
    let best_by = |name: &str, key: &dyn Fn(&Off) -> String| {
        let mut m: std::collections::BTreeMap<String, f64> = Default::default();
        for o in &scored {
            let e = m.entry(key(o)).or_insert(f64::INFINITY);
            *e = e.min(o.value);
        }
        print!("seed reads, best per {name}:");
        for (k, v) in m {
            print!(" {k}:{v:.1}");
        }
        println!();
    };
    best_by("deploy", &|o| format!("{:02}", o.deploy));
    best_by("coast", &|o| o.coast.map_or("none".into(), |c| format!("{c:02}")));
    best_by("run", &|o| format!("{:02}", o.run));
    best_by("z", &|o| format!("{:.1}", o.spot[2]));
    best_by("yaw", &|o| format!("{:.0}", o.yaw));
    std::fs::write(dir.join("runs/dome/slab.json"), serde_json::to_string(&solved).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// `slabtime`: the tick timings, then the rest around the best.
fn slabtime(dir: &Path, w: &World) -> Result<(), String> {
    let table = load_table(dir)?;
    let walls = dome_walls(w);
    let rec = Trajectory::read(&dir.join("runs/tas_gen1_34/academy.traj.jsonl"))?;
    let t0 = Instant::now();
    let solve = |o: &Off| -> Option<Off> {
        let mut seeds: Vec<Vec<U>> = vec![o.u.clone()];
        seeds.extend(dive_seeds(&walls, &table, &o.x, 3).into_iter().map(|s| s.1));
        let s = from_start(&walls, &table, o.x, &seeds)?;
        Some(Off { value: s.value, ticks: s.ticks, exit: s.exit, u: s.u, ..o.clone() })
    };
    let score = |jobs: &[([f64; 3], f64, usize, usize, usize, OffMore)]| -> Vec<Off> {
        let mut v: Vec<Off> = jobs
            .par_iter()
            .filter_map(|&(spot, yaw, run, coast, deploy, more)| {
                let x = off_state(&rec, w, spot, yaw, run, Some(coast), deploy, more)?;
                let (value, u) = dive_seeds(&walls, &table, &x, 1).into_iter().next()?;
                Some(Off { spot, yaw, run, coast: Some(coast), deploy, more, x, value, ticks: u.len(), exit: x, u })
            })
            .collect();
        v.sort_by(|a, b| a.value.total_cmp(&b.value));
        v
    };
    let line = |o: &Off| {
        let e = o.exit;
        println!(
            "{:7.2} x{:6.1} z{:7.2} yaw{:4.0} run{:2} hold{:2} back{:2}x{:2} sprint{:3} 45:{} hit:{} deploy{:3} air{:3.0} | deploy y{:6.2} z{:6.2} v{:5.2} {:5.2} | {:2} | exit y{:6.2} v{:5.2} {:5.2} {:5.2}",
            o.value, o.spot[0], o.spot[2], o.yaw, o.run, o.more.hold, o.more.back, o.more.brake, o.coast.unwrap_or(0), o.more.fwd45 as u8, o.more.contact as u8, o.deploy, o.more.air_turn, o.x[1], o.x[2], o.x[4], o.x[5], o.ticks, e[1], e[3], e[4], e[5]
        );
    };
    let best_by = |name: &str, of: &[Off], key: &dyn Fn(&Off) -> String| {
        let mut m: std::collections::BTreeMap<String, f64> = Default::default();
        for o in of {
            let e = m.entry(key(o)).or_insert(f64::INFINITY);
            *e = e.min(o.value);
        }
        print!("best per {name}:");
        for (k, v) in m {
            print!(" {k}:{v:.2}");
        }
        println!();
    };

    // stage 1, the timings, from a mid heading and spot in x
    let mut jobs = Vec::new();
    for z in [228.69, 227.9] {
        for run in 0..=5usize {
            for hold in 0..=6usize {
                for brake in (2..=34usize).step_by(2) {
                    for coast in 10..=44usize {
                        if coast <= hold + brake {
                            continue;
                        }
                        for deploy in 47..=54usize {
                            for contact in [false, true] {
                                jobs.push(([-25.5, 283.7, z], 177.0, run, coast, deploy, OffMore { hold, brake, back: 2, fwd45: true, contact, ..Default::default() }));
                            }
                        }
                    }
                }
            }
        }
    }
    let scored = score(&jobs);
    println!("stage 1: {} of {} seeds reach the door, {:.0} s", scored.len(), jobs.len(), t0.elapsed().as_secs_f64());
    // the polish is per spot, so the back wall's answer isn't crowded out by a coarse seed read
    let mut picked: Vec<&Off> = Vec::new();
    for z in [228.69, 227.9] {
        for contact in [false, true] {
            picked.extend(scored.iter().filter(|o| o.spot[2] == z && o.more.contact == contact).take(1500));
        }
    }
    let mut solved: Vec<Off> = picked.par_iter().filter_map(|o| solve(o)).collect();
    solved.sort_by(|a, b| a.value.total_cmp(&b.value));
    println!("stage 1: {} solved, {:.0} s", solved.len(), t0.elapsed().as_secs_f64());
    for o in solved.iter().take(25) {
        line(o);
    }
    best_by("spot z", &solved, &|o| format!("{:.2}", o.spot[2]));
    best_by("run", &solved, &|o| format!("{:02}", o.run));
    best_by("hold", &solved, &|o| format!("{:02}", o.more.hold));
    best_by("brake", &solved, &|o| format!("{:02}", o.more.brake));
    best_by("sprint on", &solved, &|o| format!("{:02}", o.coast.unwrap_or(0)));
    best_by("deploy", &solved, &|o| format!("{:02}", o.deploy));
    best_by("deploy z", &solved, &|o| format!("{:.0}", o.x[2]));
    best_by("contact", &solved, &|o| o.more.contact.to_string());
    std::io::stdout().flush().ok();

    // stage 2, spot and headings around the best timings
    let mut jobs = Vec::new();
    for o in solved.iter().take(40) {
        for x in [-25.8, -25.5, -25.2] {
            for yaw in [174.0, 176.0, 178.0, 180.0] {
                for air_turn in [-3.0, 0.0, 3.0] {
                    for back in [0u8, 1, 2] {
                        for fwd45 in [false, true] {
                            jobs.push(([x, 283.7, o.spot[2]], yaw, o.run, o.coast.unwrap(), o.deploy, OffMore { air_turn, back, fwd45, ..o.more }));
                        }
                    }
                }
            }
        }
    }
    let scored2 = score(&jobs);
    println!("stage 2: {} of {} seeds reach the door, {:.0} s", scored2.len(), jobs.len(), t0.elapsed().as_secs_f64());
    let mut solved2: Vec<Off> = scored2[..2500.min(scored2.len())].par_iter().filter_map(|o| solve(o)).collect();
    println!("stage 2: {} solved, {:.0} s", solved2.len(), t0.elapsed().as_secs_f64());
    solved2.extend(solved);
    solved2.sort_by(|a, b| a.value.total_cmp(&b.value));
    for o in solved2.iter().take(25) {
        line(o);
    }
    best_by("spot x", &solved2, &|o| format!("{:.1}", o.spot[0]));
    best_by("yaw", &solved2, &|o| format!("{:.0}", o.yaw));
    best_by("sprint turn", &solved2, &|o| format!("{:.0}", o.more.air_turn));
    best_by("back keys", &solved2, &|o| o.more.back.to_string());
    best_by("north on a 45", &solved2, &|o| o.more.fwd45.to_string());
    solved2.truncate(3000);
    std::fs::write(dir.join("runs/dome/slabtime.json"), serde_json::to_string(&solved2).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// `slabflights N [FILE PREFIX]`: the best `N` of `FILE`, each different in its timing, as
/// `runs/hybrid/PREFIX_K` for `optimize --hybrid`: flown under port off the slab, then the merge into the
/// 535 TAS and its states after.
fn slabflights(dir: &Path, course: &Course, w: &World, n: usize, file: &str, prefix: &str) -> Result<(), String> {
    let r = reference_from(dir, course, "runs/tas_gen1_34/academy.traj.jsonl")?;
    let found: Vec<Off> = serde_json::from_str(&std::fs::read_to_string(dir.join(file)).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut chosen: Vec<&Off> = Vec::new();
    for o in &found {
        let timing = |c: &Off| (c.deploy, c.coast, c.run, c.more.hold, c.more.brake, c.more.back, c.more.contact, c.spot[2].to_bits());
        if chosen.iter().all(|c| timing(c) != timing(o)) {
            chosen.push(o);
            if chosen.len() == n {
                break;
            }
        }
    }
    let pts: Vec<[f64; 3]> = r.rec.rows.iter().map(|x| x.state.position).collect();
    let walls = Walls::around(w, &pts, 8.0, 1.0);
    let keys = r.rec.rows[r.exit].input.keys.replace('J', "");
    let out = dir.join("runs/hybrid");
    let total = r.x.len() - 1;
    let lines: Vec<String> = chosen.par_iter().enumerate().map(|(k, o)| -> Result<String, String> {
        let mut initial = r.rec.initial.clone();
        initial.position = o.spot;
        initial.delta_movement = [0.0, 0.0, 0.0];
        initial.y_rot = o.yaw;
        initial.x_rot = 0.0;
        initial.on_ground = false;
        initial.main_supporting_block_pos = None;
        let mut inputs = off_inputs(o.yaw, o.run, o.coast, o.deploy, o.more);
        let pb = Problem { kernel: Generic::default(), walls: &walls, x0: o.x, goal: Goal::Plane { normal: [0.0, 0.0, -1.0], at: -EXIT_Z, cap: o.u.len() + 5 }, opts: opts() };
        let xs = pb.rollout(&o.u);
        let mut yaw = o.yaw;
        for (t, u) in o.u.iter().enumerate() {
            let (input, y) = input_at(&xs[t], u, yaw);
            yaw = y;
            inputs.push(InputRow { yaw: input.yaw as f64, pitch: input.pitch as f64, keys: keys.clone() });
        }
        let mut h = r.rec.header.clone();
        h.source = format!("{prefix}{k}: x {:.2} z {:.2} yaw {:.0} run {} {:?} sprint {:?} deploy {}", o.spot[0], o.spot[2], o.yaw, o.run, o.more, o.coast, o.deploy);
        let (head, stopped) = fly(&initial, &inputs, &[], w, h.clone());
        if let Some((t, e)) = stopped {
            return Ok(format!("{prefix}{k}: port stopped at {t}: {e:?}"));
        }
        let exit = (1..=head.rows.len()).find(|&t| state_at(&head, t).position[2] < EXIT_Z).ok_or("no exit")?;
        let s = flight(state_at(&head, exit));
        let Some(join) = merge(&walls, &r, &s, MERGE_T) else {
            return Ok(format!("{prefix}{k}: no merge from {s:?}"));
        };
        let mut rows = head.rows[..exit].to_vec();
        let proto = r.rec.rows[r.exit].clone();
        let mut near = rows.last().map_or(yaw, |x| x.input.yaw);
        for (t, u) in join.u.iter().enumerate() {
            let (input, y) = input_at(&join.x[t], u, near);
            near = y;
            let mut row = proto.clone();
            row.t = rows.len() + 1;
            row.input.yaw = input.yaw as f64;
            row.input.pitch = input.pitch as f64;
            let e = join.x[t + 1];
            row.state.position = [e[0], e[1], e[2]];
            row.state.delta_movement = [e[3], e[4], e[5]];
            rows.push(row);
        }
        for t in r.exit + MERGE_T..r.rec.rows.len() {
            let mut row = r.rec.rows[t].clone();
            row.t = rows.len() + 1;
            rows.push(row);
        }
        let traj = Trajectory { header: h, initial, arrivals: Vec::new(), rows };
        traj.write(&out.join(format!("{prefix}{k}.traj.jsonl")))?;
        Ok(format!(
            "{prefix}{k}: table {:.2}, sprint {:?} deploy {} yaw {:.0}: exit y {:.2} v {:.3} {:.3} {:.3}, merges in {} (reference {}) for {} timed, flown exit tick {exit}",
            o.value, o.coast, o.deploy, o.yaw, s[1], s[3], s[4], s[5], join.u.len(), MERGE_T, join.u.len() + total - MERGE_T
        ))
    }).collect::<Result<_, _>>()?;
    for l in lines {
        println!("{l}");
    }
    Ok(())
}

/// `slabcheck NAME...`: flights in `runs/relax`, by what `tas` holds a plan to: `verify`, and the
/// server's copy of the player clear of walls from the deploy to the finish.
fn slabcheck(dir: &Path, course: &Course, w: &World, names: &[String]) -> Result<(), String> {
    for name in names {
        let t = Trajectory::read(&dir.join(format!("runs/relax/{name}.traj.jsonl")))?;
        let r = etas::verify::verify(course, w, etas::verify::Plan::Trajectory(&t));
        let (start, end) = r.timer_tick().zip(r.finish_tick()).ok_or("no timer or finish")?;
        let pts: Vec<[f64; 3]> = t.rows.iter().map(|x| x.state.position).collect();
        let walls = Walls::around(w, &pts, 8.0, 1.0);
        let flown: Vec<X> = t.rows.iter().map(|x| flight(&x.state)).collect();
        let deploy = t.rows.iter().position(|x| x.state.fall_flying).ok_or("no deploy")?;
        let hits: Vec<_> = etas::solve::ddp::server_hits(&walls, &flown).into_iter().filter(|&(i, _)| i >= deploy && t.rows[i].t < end).map(|(i, c)| (t.rows[i].t, c)).collect();
        println!("{name}: {} timed {} (timer {start}, finish {end}), server hits {hits:?}, failures {:?}", if r.ok() { "PASS" } else { "FAIL" }, end - start, r.failures.first());
    }
    Ok(())
}

/// `slabdive`: the same late pull-out from the current start, with no landing: from ticks of the 535
/// TAS's climb, turn north at some pitch, dive to a height, and pull out, scored on the table and the
/// best solved. why not a reach to a slab flight's state? its velocity is too exact a target to say
/// anything: all 125 reaches failed.
fn slabdive(dir: &Path, w: &World) -> Result<(), String> {
    let table = load_table(dir)?;
    let rec = Trajectory::read(&dir.join("runs/tas_gen1_34/academy.traj.jsonl"))?;
    let walls = dome_walls(w);
    let k = Generic::default();
    let aim = |x: &X, to: [f64; 2]| -> f64 {
        let want = (-(to[0] - x[0])).atan2(to[1] - x[2]);
        let have = (-x[3]).atan2(x[5]);
        (want - have + PI).rem_euclid(2.0 * PI) - PI
    };
    let mut jobs = Vec::new();
    for from in [22usize, 24, 26, 28, 30, 33, 36, 40, 46] {
        for turn in [0usize, 4, 8, 12, 16, 20, 26, 32] {
            for tilt in [20.0f64, 35.0, 50.0, 65.0, 80.0] {
                for low in (213..=249).step_by(3) {
                    for pull in [-10.0f64, -4.0, 0.0, 4.0, 8.0, 14.0, 22.0] {
                        jobs.push((from, turn, tilt, low as f64, pull));
                    }
                }
            }
        }
    }
    let mut scored: Vec<(f64, usize, Vec<U>)> = jobs.par_iter().filter_map(|&(from, turn, tilt, low, pull)| {
        let mut x = flight(state_at(&rec, from));
        let mut u = Vec::new();
        for t in 0..HEAD_CAP {
            let c: U = if t < turn {
                [aim(&x, [DOOR_X, 205.0]), tilt.to_radians()]
            } else if x[1] > low {
                [aim(&x, [DOOR_X, 205.0]), 89f64.to_radians()]
            } else {
                [aim(&x, [DOOR_X, 205.0]), pull.to_radians()]
            };
            let (n, corners) = step(&k, &x, c);
            if corners.windows(2).any(|l| walls.clearance(l[0], l[1]).0 < MARGIN) {
                return None;
            }
            u.push(c);
            if let Some(v) = exit_read(&table, &x, &n) {
                return Some((v, from, u));
            }
            x = n;
        }
        None
    }).collect();
    println!("{} of {} seeds reach the door", scored.len(), jobs.len());
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (v, f, u) in scored.iter().take(5) {
        println!("seed reads {v:.2}, from tick {f}, {} ticks", u.len());
    }
    let mut got: Vec<(f64, usize, Start)> = scored.iter().take(200).collect::<Vec<_>>().par_iter().filter_map(|(_, f, u)| {
        let s = from_start(&walls, &table, flight(state_at(&rec, *f)), std::slice::from_ref(u))?;
        Some((s.value, *f, s))
    }).collect();
    got.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (v, f, s) in got.iter().take(10) {
        let e = s.exit;
        println!("solved {v:.2} from tick {f} in {}: exit y {:.2} v {:.3} {:.3} {:.3}", s.ticks, e[1], e[3], e[4], e[5]);
    }
    if let Some((_, f, s)) = got.first() {
        let xs: Vec<X> = { let pb = Problem { kernel: k, walls: &walls, x0: s.x, goal: Goal::Plane { normal: [0.0, 0.0, -1.0], at: -EXIT_Z, cap: s.u.len() + 5 }, opts: opts() }; pb.rollout(&s.u) };
        let low = xs.iter().position(|x| x[1] < 225.0).unwrap_or(0);
        println!("best from tick {f}: at y<225, z {:.2} v {:.3} {:.3} {:.3}, top y {:.2}", xs[low][2], xs[low][3], xs[low][4], xs[low][5], xs.iter().map(|x| x[1]).fold(f64::MIN, f64::max));
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (course, world) = Course::load(dir)?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|a| a.as_str()) {
        Some("slab") => slab(dir, &world),
        Some("slabtime") => slabtime(dir, &world),
        Some("slabflights") => slabflights(
            dir,
            &course,
            &world,
            args.get(1).map_or(Ok(6), |a| a.parse()).map_err(|e: std::num::ParseIntError| e.to_string())?,
            args.get(2).map_or("runs/dome/slab.json", |a| a.as_str()),
            args.get(3).map_or("slab_", |a| a.as_str()),
        ),
        Some("slabdive") => slabdive(dir, &world),
        Some("slabcheck") => slabcheck(dir, &course, &world, &args[1..]),
        Some("views") => views(dir, &course, &world),
        Some("samples") => samples(dir, &course, &world, &args[1..]),
        Some("starts") => starts(dir, &world),
        Some("results") => results(dir, &world),
        Some("global") => global(dir, &world),
        Some("frontier") => frontier(dir, &world),
        Some("flights") => flights(dir, &course, &world, args.get(1).map_or(Ok(3), |a| a.parse()).map_err(|e: std::num::ParseIntError| e.to_string())?),
        p => Err(format!("{p:?}?")),
    }
}
