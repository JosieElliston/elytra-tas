//! Phase `routes`: other routes through the course, split at gates (`etas::solve::gate`) from
//! `tools/route_gates.py`'s `runs/routes/SET.json`.
//!
//! - `tables`: from the last stretch back, the value of each gate's table. Each stretch is first
//!   solved from the reference flight's crossing along every route: the recorded ones, and ones a voxel search
//!   finds. A route gives its first guesses by [`guided`]. The best distinct routes then seed every
//!   cell, and each cell tries its better neighbors' controls. Writes `runs/routes/SET/seg_N.json`,
//!   and skips a stretch that already has one.
//! - `forward`: from a flight's exit, each stretch against its table, keeping the best `BEAM`
//!   crossings; writes the best whole flights as `runs/hybrid/SET_N.traj.jsonl` for
//!   `optimize --hybrid`.
//!
//! - `flat`: `forward` with no tables, each stretch reaching its gate soonest, as
//!   `runs/hybrid/SET_flat_N`. why? a table's read didn't rank flights: academy's forecast best
//!   polished to 552 and one near its worst to 540 (2026-11-05).
//! - `start`: `tools/start_states.py`'s states against the first gate's table, with the ticks before
//!   the timer free.
//!
//! `cargo run --release -p academy --bin routes -- SET tables|forward|flat|start [FLIGHT]`

use std::collections::BinaryHeap;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use etas::course::Course;
use etas::solve::bounces::{flight, state_at};
use etas::solve::ddp::{Goal, Opts, Plan, Problem, U, X, step};
use etas::solve::gate::{AXES, Gate, ValueTable};
use etas::solve::walls::Walls;
use etas::trajectory::Trajectory;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sim::generic::Generic;
use world::World;

const MARGIN: f64 = 0.02;
/// The clearance a solve is taken at, short of `MARGIN` (`optimize`'s reason).
const LOOSE: f64 = 0.003;
/// [`guided`]'s seeds: ticks a control is held to judge it, and the price of a block off the path.
const AHEADS: [usize; 3] = [6, 10, 16];
const CROSSES: [f64; 2] = [0.3, 1.0];
const CELL_AHEAD: usize = 10;
/// Its controls: leads in steps either way, and pitches, degrees.
const LEADS: i32 = 6;
const LEAD_STEP: f64 = 0.2;
const PITCHES: [f64; 8] = [-30.0, -10.0, 0.0, 10.0, 20.0, 35.0, 55.0, 80.0];
/// The clearance it keeps, and what each tick lost to a hit costs, in blocks along the path.
const SEED_CLEAR: f64 = 0.05;
const HIT: f64 = 4.0;
/// Distinct routes that seed the whole table, and how far apart they must be somewhere.
const TOP: usize = 3;
const DISTINCT: f64 = 2.5;
/// Voxel routes per stretch: how much a used corridor costs afterward, how wide it is, and what
/// a block of climbing costs on top of a block of travel. why dear? climbing spends speed.
const VOXEL_ROUTES: usize = 6;
const VOXEL_REUSE: f64 = 3.0;
const VOXEL_WIDTH: f64 = 3.5;
const VOXEL_CLIMB: f64 = 4.0;
const VOXEL_PAD: f64 = 10.0;
/// Sweeps of the table, neighbors a cell tries in each, and how much better one has to be to bother.
const SWEEPS: usize = 2;
const NEIGHBORS: usize = 2;
const WORTH: f64 = 0.01;
/// The most a cell reads over a neighbor, ticks. A cell's own answer can be poor as easily as the
/// cell is: 90 ticks apart on academy's `t487`, with neighbors within a few.
const STEP_LIMIT: f64 = 6.0;
const BEAM: usize = 24;
/// The most of a beam that may share a stretch's route.
const PER_ROUTE: usize = 6;
const COAST: usize = 5;
/// Routes a beam entry goes on by.
const BRANCH: usize = 5;

#[derive(Deserialize)]
struct GateJ {
    name: String,
    axis: usize,
    sign: f64,
    at: f64,
    lo: [f64; 2],
    hi: [f64; 2],
    nodes: [Vec<f64>; AXES],
    /// The reference flight's crossing, in table coordinates.
    reference: [f64; AXES],
}
impl GateJ {
    fn gate(&self) -> Gate {
        Gate {
            axis: self.axis,
            sign: self.sign,
            at: self.at,
            lo: self.lo,
            hi: self.hi,
        }
    }
}
#[derive(Deserialize)]
struct RouteJ {
    label: String,
    xyz: Vec<[f64; 3]>,
}
#[derive(Deserialize)]
struct SegJ {
    from: String,
    to: String,
    routes: Vec<RouteJ>,
}
#[derive(Deserialize)]
struct SetJ {
    gates: Vec<GateJ>,
    segments: Vec<SegJ>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Class {
    label: String,
    value: f64,
    u: Vec<U>,
}
#[derive(Serialize, Deserialize)]
struct SegOut {
    values: Vec<f64>,
    reachable: usize,
    classes: Vec<Class>,
    /// Every route's best from the reference's crossing: label, value if any, good seeds of all
    /// tried. why an Option? JSON has no infinity: it writes null and can't read it back as a float.
    tried: Vec<(String, Option<f64>, usize, usize)>,
    secs: f64,
}

fn opts() -> Opts {
    Opts {
        tol: 1e-6,
        margin: MARGIN,
        server: true,
        ..Opts::default()
    }
}

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

/// A plan clear of walls it can be sure of, the goal met.
fn good(pb: &Problem, plan: &Plan) -> bool {
    let loose = Problem {
        opts: Opts {
            margin: MARGIN - LOOSE,
            ..pb.opts.clone()
        },
        ..pb.clone()
    };
    loose.feasible(plan) && plan.x.iter().all(|x| pb.walls.covers([x[0], x[1], x[2]]))
}

/// A first guess along `path` from `x0` that stays out of walls: per tick, the control that, held
/// for `ahead` ticks, gets furthest along the path, less `cross` per block off it, and hits nothing
/// for longest. why not pure pursuit? a seed inside a wall stays there: the solve's gradient inside
/// points along the wall's thickness, and every price lands on the same clearance, -0.2 to -0.7 on
/// academy's last stretch (2026-11-05), so every seed of every route failed.
fn guided(
    walls: &Walls,
    x0: &X,
    path: &[[f64; 3]],
    ahead: usize,
    cross: f64,
    cap: usize,
) -> Vec<U> {
    let k = Generic::default();
    let d = |a: &[f64; 3], b: &[f64; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let mut arc = vec![0.0];
    for w in path.windows(2) {
        arc.push(arc.last().unwrap() + d(&w[0], &w[1]));
    }
    let nearest = |p: &[f64; 3], from: usize, span: usize| -> usize {
        (from..(from + span).min(path.len()))
            .min_by(|&a, &b| d(&path[a], p).total_cmp(&d(&path[b], p)))
            .unwrap_or(from)
    };
    let mut x = *x0;
    let mut at = 0usize;
    let mut u = Vec::new();
    while u.len() < cap && at + 1 < path.len() {
        let mut best = (f64::NEG_INFINITY, [0.0, 0.0]);
        for lead_i in -LEADS..=LEADS {
            for &pitch in &PITCHES {
                let c = [lead_i as f64 * LEAD_STEP, pitch.to_radians()];
                let mut s = x;
                let mut j = at;
                let mut score = f64::NEG_INFINITY;
                let mut clear = 0;
                for t in 0..ahead {
                    let (n, corners) = step(&k, &s, c);
                    let hit = corners
                        .windows(2)
                        .any(|l| walls.clearance(l[0], l[1]).0 < SEED_CLEAR);
                    if hit {
                        break;
                    }
                    s = n;
                    let p = [s[0], s[1], s[2]];
                    j = nearest(&p, j, 16);
                    clear = t + 1;
                    score = arc[j] - cross * d(&path[j], &p);
                }
                // what it reaches before a hit, and a hit soon is worse than one late
                let total = score - HIT * (ahead - clear) as f64;
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

/// Diverse ways from `a` to `b` through free cells: each a least-cost path on a block grid, with
/// the corridors of the ones before it made dearer.
fn voxel_routes(
    walls: &Walls,
    lo: [f64; 3],
    hi: [f64; 3],
    a: [f64; 3],
    b: [f64; 3],
) -> Vec<Vec<[f64; 3]>> {
    let dims: [usize; 3] = std::array::from_fn(|i| (hi[i] - lo[i]).ceil() as usize + 1);
    let idx = |c: [usize; 3]| (c[0] * dims[1] + c[1]) * dims[2] + c[2];
    let total = dims[0] * dims[1] * dims[2];
    let free: Vec<bool> = (0..total)
        .into_par_iter()
        .map(|i| {
            let c = [i / dims[2] / dims[1], i / dims[2] % dims[1], i % dims[2]];
            let p: [f64; 3] = std::array::from_fn(|k| lo[k] + c[k] as f64);
            walls.clearance(p, p).0 >= 0.1
        })
        .collect();
    let cell = |p: [f64; 3]| -> Option<usize> {
        let mut best = None;
        let mut bd = f64::INFINITY;
        let base: [i64; 3] = std::array::from_fn(|k| (p[k] - lo[k]).round() as i64);
        for dx in -3i64..=3 {
            for dy in -3i64..=3 {
                for dz in -3i64..=3 {
                    let c = [base[0] + dx, base[1] + dy, base[2] + dz];
                    if (0..3).any(|k| c[k] < 0 || c[k] >= dims[k] as i64) {
                        continue;
                    }
                    let u = [c[0] as usize, c[1] as usize, c[2] as usize];
                    let d = (dx * dx + dy * dy + dz * dz) as f64;
                    if free[idx(u)] && d < bd {
                        bd = d;
                        best = Some(idx(u));
                    }
                }
            }
        }
        best
    };
    let (Some(s), Some(g)) = (cell(a), cell(b)) else {
        return Vec::new();
    };
    let mut reuse = vec![1.0f64; total];
    let mut out = Vec::new();
    for _ in 0..VOXEL_ROUTES {
        let mut dist = vec![f64::INFINITY; total];
        let mut prev = vec![usize::MAX; total];
        dist[s] = 0.0;
        #[derive(PartialEq)]
        struct Q(f64, usize);
        impl Eq for Q {}
        impl Ord for Q {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                o.0.total_cmp(&self.0)
            }
        }
        impl PartialOrd for Q {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        let mut q = BinaryHeap::from([Q(0.0, s)]);
        while let Some(Q(d, i)) = q.pop() {
            if d > dist[i] {
                continue;
            }
            if i == g {
                break;
            }
            let c = [
                (i / dims[2] / dims[1]) as i64,
                (i / dims[2] % dims[1]) as i64,
                (i % dims[2]) as i64,
            ];
            for dx in -1i64..=1 {
                for dy in -1i64..=1 {
                    for dz in -1i64..=1 {
                        let n = [c[0] + dx, c[1] + dy, c[2] + dz];
                        if (0..3).any(|k| n[k] < 0 || n[k] >= dims[k] as i64) {
                            continue;
                        }
                        let j = idx([n[0] as usize, n[1] as usize, n[2] as usize]);
                        if !free[j] {
                            continue;
                        }
                        let len = ((dx * dx + dy * dy + dz * dz) as f64).sqrt();
                        let nd = d + reuse[j] * len + VOXEL_CLIMB * dy.max(0) as f64;
                        if nd < dist[j] {
                            dist[j] = nd;
                            prev[j] = i;
                            q.push(Q(nd, j));
                        }
                    }
                }
            }
        }
        if !dist[g].is_finite() {
            break;
        }
        let mut path = Vec::new();
        let mut i = g;
        while i != usize::MAX {
            let c = [i / dims[2] / dims[1], i / dims[2] % dims[1], i % dims[2]];
            path.push(std::array::from_fn::<f64, 3, _>(|k| lo[k] + c[k] as f64));
            i = prev[i];
        }
        path.reverse();
        // dearer around it
        let w = VOXEL_WIDTH.ceil() as i64;
        for p in &path {
            let c: [i64; 3] = std::array::from_fn(|k| (p[k] - lo[k]) as i64);
            for dx in -w..=w {
                for dy in -w..=w {
                    for dz in -w..=w {
                        let n = [c[0] + dx, c[1] + dy, c[2] + dz];
                        if (0..3).any(|k| n[k] < 0 || n[k] >= dims[k] as i64)
                            || ((dx * dx + dy * dy + dz * dz) as f64) > VOXEL_WIDTH * VOXEL_WIDTH
                        {
                            continue;
                        }
                        let j = idx([n[0] as usize, n[1] as usize, n[2] as usize]);
                        reuse[j] = VOXEL_REUSE;
                    }
                }
            }
        }
        // the grid's stairs out
        let smooth: Vec<[f64; 3]> = (0..path.len())
            .map(|i| {
                let a = i.saturating_sub(3);
                let b = (i + 4).min(path.len());
                std::array::from_fn(|k| {
                    path[a..b].iter().map(|p| p[k]).sum::<f64>() / (b - a) as f64
                })
            })
            .collect();
        out.push(smooth);
    }
    out
}

fn resampled(x: &[[f64; 3]]) -> Vec<[f64; 3]> {
    const N: usize = 30;
    let mut d = vec![0.0];
    for w in x.windows(2) {
        let e = ((w[0][0] - w[1][0]).powi(2)
            + (w[0][1] - w[1][1]).powi(2)
            + (w[0][2] - w[1][2]).powi(2))
        .sqrt();
        d.push(d.last().unwrap() + e);
    }
    let mut j = 0;
    (0..N)
        .map(|k| {
            let s = d.last().unwrap() * k as f64 / (N - 1) as f64;
            while j + 2 < d.len() && d[j + 1] < s {
                j += 1;
            }
            let w = if d[j + 1] > d[j] {
                (s - d[j]) / (d[j + 1] - d[j])
            } else {
                0.0
            };
            std::array::from_fn(|m| x[j][m] + w * (x[j + 1][m] - x[j][m]))
        })
        .collect()
}

fn apart(a: &[[f64; 3]], b: &[[f64; 3]]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(p, q)| {
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
        })
        .fold(0.0, f64::max)
}

fn positions(x: &[X]) -> Vec<[f64; 3]> {
    x.iter().map(|s| [s[0], s[1], s[2]]).collect()
}

struct Stretch<'a> {
    walls: Walls,
    goal: Goal,
    routes: Vec<(String, Vec<[f64; 3]>)>,
    set: &'a SetJ,
    cap: usize,
}

/// The time from a state on, its plan if it's good.
fn solve_from(st: &Stretch, x0: X, seed: &[U]) -> Option<(f64, Plan)> {
    let pb = Problem {
        kernel: Generic::default(),
        walls: &st.walls,
        x0,
        goal: st.goal.clone(),
        opts: opts(),
    };
    let s = pb.solve(seed);
    if std::env::var_os("ROUTES_DEBUG").is_some() {
        let e = s.plan.x.last().unwrap();
        eprintln!(
            "x0 {:?} seed {} n {} value {:.2} worst {:.3} covered {} end {:?} good {} stages {:?}",
            &x0[..3],
            seed.len(),
            s.plan.u.len(),
            pb.value(&s.plan.x),
            pb.worst(&s.plan.x, &s.plan.u),
            s.plan.x.iter().all(|x| pb.walls.covers([x[0], x[1], x[2]])),
            &e[..3],
            good(&pb, &s.plan),
            s.stages
                .iter()
                .map(|t| (t.value as i64, (t.worst * 1000.0) as i64, t.iters))
                .collect::<Vec<_>>()
        );
    }
    good(&pb, &s.plan).then(|| (pb.value(&s.plan.x), s.plan))
}

fn stretch<'a>(
    world: &World,
    course: &Course,
    set: &'a SetJ,
    i: usize,
    next: Option<Arc<ValueTable>>,
) -> Result<Stretch<'a>, String> {
    let seg = &set.segments[i];
    let mut pts: Vec<[f64; 3]> = seg
        .routes
        .iter()
        .flat_map(|r| r.xyz.iter().copied())
        .collect();
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &pts {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    for k in 0..3 {
        lo[k] -= VOXEL_PAD;
        hi[k] += VOXEL_PAD;
    }
    pts.push(lo);
    pts.push(hi);
    let walls = Walls::around(world, &pts, 4.0, 1.0);
    let longest = seg.routes.iter().map(|r| r.xyz.len()).max().unwrap_or(100);
    let cap = longest.min(400) + 60;
    let goal = match next {
        Some(table) => Goal::Table {
            table,
            cap,
            time_weight: 1.0,
        },
        None => finish_goal(course, cap)?,
    };
    let mut routes: Vec<(String, Vec<[f64; 3]>)> = seg
        .routes
        .iter()
        .filter(|r| r.xyz.len() >= 3)
        .map(|r| (r.label.clone(), r.xyz.clone()))
        .collect();
    // the voxel routes, between the first recorded route's ends
    if let Some(first) = seg.routes.first() {
        let (a, b) = (first.xyz[0], *first.xyz.last().unwrap());
        for (k, path) in voxel_routes(&walls, lo, hi, a, b).into_iter().enumerate() {
            let r = resampled(&path);
            if routes
                .iter()
                .all(|(_, q)| apart(&resampled(q), &r) >= DISTINCT)
            {
                routes.push((format!("voxel{k}"), path));
            }
        }
    }
    Ok(Stretch {
        walls,
        goal,
        routes,
        set,
        cap,
    })
}

fn seeds(st: &Stretch, x0: &X, path: &[[f64; 3]]) -> Vec<Vec<U>> {
    let mut out = Vec::new();
    for ahead in AHEADS {
        for cross in CROSSES {
            out.push(guided(&st.walls, x0, path, ahead, cross, st.cap));
        }
    }
    out
}

/// Every route from one state: per route the seeds' best, and how many seeds came to anything.
fn all_routes(st: &Stretch, x0: X) -> Vec<(String, Option<(f64, Plan)>, usize, usize)> {
    st.routes
        .par_iter()
        .map(|(label, path)| {
            let tried = seeds(st, &x0, path);
            let n = tried.len();
            let got: Vec<(f64, Plan)> = tried
                .par_iter()
                .filter_map(|u| solve_from(st, x0, u))
                .collect();
            let m = got.len();
            let best = got.into_iter().min_by(|a, b| a.0.total_cmp(&b.0));
            (label.clone(), best, m, n)
        })
        .collect()
}

/// The best routes, distinct ones only.
fn distinct(mut found: Vec<(String, f64, Plan)>, top: usize) -> Vec<(String, f64, Plan)> {
    found.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut out: Vec<(String, f64, Plan, Vec<[f64; 3]>)> = Vec::new();
    for (l, v, p) in found {
        let r = resampled(&positions(&p.x));
        if out.iter().all(|o| apart(&o.3, &r) >= DISTINCT) {
            out.push((l, v, p, r));
            if out.len() == top {
                break;
            }
        }
    }
    out.into_iter().map(|(l, v, p, _)| (l, v, p)).collect()
}

fn table_of(g: &GateJ, seg: &SegOut) -> ValueTable {
    ValueTable {
        gate: g.gate(),
        nodes: g.nodes.clone(),
        values: seg.values.clone(),
    }
}

fn load(path: &Path) -> Result<Option<SegOut>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&s)
        .map(Some)
        .map_err(|e| e.to_string())
}

fn tables(
    dir: &Path,
    name: &str,
    set: &SetJ,
    course: &Course,
    world: &World,
) -> Result<(), String> {
    let out = dir.join("runs/routes").join(name);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut next: Option<Arc<ValueTable>> = None;
    for i in (0..set.segments.len()).rev() {
        let file = out.join(format!("seg_{i}.json"));
        let g = &set.gates[i];
        if let Some(done) = load(&file)? {
            println!("{}: have it", g.name);
            next = Some(Arc::new(table_of(g, &done)));
            continue;
        }
        let t0 = Instant::now();
        let st = stretch(world, course, set, i, next.clone())?;
        let seg = &st.set.segments[i];
        println!(
            "{} -> {}: {} routes ({:.0} s to set up)",
            seg.from,
            seg.to,
            st.routes.len(),
            t0.elapsed().as_secs_f64()
        );
        let gate = g.gate();
        // from the reference's own crossing. why not the middle cell? with another tau a tight
        // gate's cell can start inside a wall (-0.2 on academy's t505), which no route gets out of
        let x_mid = gate.state(&g.reference);
        let tau_mid = g.reference[5];
        let results = all_routes(&st, x_mid);
        let mut tried = Vec::new();
        let mut found = Vec::new();
        for (l, best, m, n) in results {
            let v = best.as_ref().map(|b| b.0 + tau_mid);
            tried.push((l.clone(), v, m, n));
            if let Some((v, p)) = best {
                found.push((l, v, p));
            }
        }
        tried.sort_by(|a, b| {
            a.1.unwrap_or(f64::INFINITY)
                .total_cmp(&b.1.unwrap_or(f64::INFINITY))
        });
        for t in &tried {
            let v = t.1.map_or("-".to_string(), |v| format!("{v:.2}"));
            println!("  {:<24} {v:>8}  {}/{} seeds", t.0, t.2, t.3);
        }
        let top = distinct(found, TOP);
        println!(
            "  seeding the table from {:?} ({:.0} s)",
            top.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(),
            t0.elapsed().as_secs_f64()
        );
        if top.is_empty() {
            return Err(format!("{}: no route solved from the middle cell", g.name));
        }
        let cells = ValueTable::cells(&g.nodes);
        let coords =
            |c: &[usize; AXES]| -> [f64; AXES] { std::array::from_fn(|a| g.nodes[a][c[a]]) };
        // a cell's start must itself be clear; then its best of the top routes
        let mut got: Vec<Option<(f64, Vec<U>)>> = cells
            .par_iter()
            .map(|c| {
                let k = coords(c);
                let x0 = gate.state(&k);
                let p = [x0[0], x0[1], x0[2]];
                if st.walls.clearance(p, p).0 < MARGIN {
                    return None;
                }
                // a route's controls from another state often fly into a wall, so its path too
                top.iter()
                    .flat_map(|(_, _, plan)| {
                        let path = positions(&plan.x);
                        [
                            plan.u.clone(),
                            guided(&st.walls, &x0, &path, CELL_AHEAD, CROSSES[0], st.cap),
                        ]
                    })
                    .filter_map(|u| solve_from(&st, x0, &u))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(v, plan)| (v + k[5], plan.u))
            })
            .collect();
        let n1 = got.iter().flatten().count();
        let walled = cells
            .iter()
            .filter(|c| {
                let x = gate.state(&coords(c));
                st.walls.clearance([x[0], x[1], x[2]], [x[0], x[1], x[2]]).0 < MARGIN
            })
            .count();
        println!(
            "  first fill: {n1}/{} cells, {walled} start in a wall ({:.0} s)",
            cells.len(),
            t0.elapsed().as_secs_f64()
        );
        // what neighbors know
        let index = |c: &[usize; AXES]| ValueTable::index(&g.nodes, c);
        let neighbors = |c: &[usize; AXES]| -> Vec<usize> {
            let mut out = Vec::new();
            for a in 0..AXES {
                for d in [-1i64, 1] {
                    let j = c[a] as i64 + d;
                    if j >= 0 && j < g.nodes[a].len() as i64 {
                        let mut n = *c;
                        n[a] = j as usize;
                        out.push(index(&n));
                    }
                }
            }
            out
        };
        let mut n2 = 0;
        for _ in 0..SWEEPS {
            let improved: Vec<(usize, f64, Vec<U>)> = cells
                .par_iter()
                .enumerate()
                .filter_map(|(ci, c)| {
                    let k = coords(c);
                    let x0 = gate.state(&k);
                    let p = [x0[0], x0[1], x0[2]];
                    if st.walls.clearance(p, p).0 < MARGIN {
                        return None;
                    }
                    let own = got[ci].as_ref().map_or(f64::INFINITY, |g| g.0);
                    let mut near: Vec<(f64, &Vec<U>, usize)> = neighbors(c)
                        .into_iter()
                        .filter_map(|n| got[n].as_ref().map(|g| (g.0, &g.1, n)))
                        .filter(|g| g.0 < own - WORTH)
                        .collect();
                    near.sort_by(|a, b| a.0.total_cmp(&b.0));
                    near.truncate(NEIGHBORS);
                    let best = near
                        .iter()
                        .flat_map(|(_, u, n)| {
                            // its controls, and its path from here
                            let them = gate.state(&coords(&cells[*n]));
                            let path = positions(
                                &Problem {
                                    kernel: Generic::default(),
                                    walls: &st.walls,
                                    x0: them,
                                    goal: st.goal.clone(),
                                    opts: opts(),
                                }
                                .rollout(u),
                            );
                            [
                                (*u).clone(),
                                guided(&st.walls, &x0, &path, CELL_AHEAD, CROSSES[0], st.cap),
                            ]
                        })
                        .filter_map(|u| solve_from(&st, x0, &u))
                        .min_by(|a, b| a.0.total_cmp(&b.0))?;
                    let v = best.0 + k[5];
                    (v < own - 1e-6).then_some((ci, v, best.1.u))
                })
                .collect();
            n2 += improved.len();
            for (ci, v, u) in improved {
                got[ci] = Some((v, u));
            }
        }
        let reachable = got.iter().flatten().count();
        let lowest = got
            .iter()
            .flatten()
            .map(|g| g.0)
            .fold(f64::INFINITY, f64::min);
        // a cell with no answer takes its neighbors'. why not a price? the walls keep a solve out of
        // a walled cell already, and a price compounds up the course: worst + 2 at each gate read 772
        // ticks at academy's exit for a 537 tick flight. The forward pass flies whatever the table
        // suggests, so a cell that reads too low costs a try, and one too high hides a route.
        let mut values: Vec<f64> = got
            .iter()
            .map(|g| g.as_ref().map_or(f64::INFINITY, |g| g.0))
            .collect();
        for _ in 0..cells.len() {
            let mut next = values.clone();
            for (ci, c) in cells.iter().enumerate() {
                let bound = neighbors(c)
                    .iter()
                    .map(|&n| values[n])
                    .fold(f64::INFINITY, f64::min)
                    + STEP_LIMIT;
                next[ci] = next[ci].min(bound);
            }
            if next == values {
                break;
            }
            values = next;
        }
        let worst = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let middle = values[ValueTable::index(&g.nodes, &[1; AXES])];
        println!(
            "  {reachable} reachable of {}, {n2} improved by neighbors; value {lowest:.2} .. {worst:.2}, middle {middle:.2} ({:.0} s)",
            cells.len(),
            t0.elapsed().as_secs_f64()
        );
        let result = SegOut {
            reachable,
            values,
            classes: top
                .iter()
                .map(|(label, v, p)| Class {
                    label: label.clone(),
                    value: *v + tau_mid,
                    u: p.u.clone(),
                })
                .collect(),
            tried,
            secs: t0.elapsed().as_secs_f64(),
        };
        std::fs::write(
            &file,
            serde_json::to_string(&result).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        next = Some(Arc::new(table_of(g, &result)));
    }
    Ok(())
}

#[derive(Clone)]
struct Partial {
    x: Vec<X>,
    u: Vec<U>,
    labels: Vec<String>,
    /// Ticks flown plus the table where it stands.
    predicted: f64,
}

fn forward(
    dir: &Path,
    name: &str,
    set: &SetJ,
    course: &Course,
    world: &World,
    flight_name: &str,
    flat: bool,
) -> Result<(), String> {
    let out = dir.join("runs/routes").join(name);
    let file = dir.join(format!("runs/{flight_name}.traj.jsonl"));
    let rec = Trajectory::read(&file)?;
    let start = &course.region("start_area")?.working;
    let exit = (1..rec.rows.len())
        .find(|&t| !start.contains(state_at(&rec, t).position))
        .ok_or("the flight never leaves the start area")?;
    let x0 = flight(state_at(&rec, exit));
    println!("from {flight_name}, tick {exit}: {x0:?}");
    let tag = if flat {
        format!("{name}_flat")
    } else {
        name.to_string()
    };
    let mut beam = vec![Partial {
        x: vec![x0],
        u: Vec::new(),
        labels: Vec::new(),
        predicted: 0.0,
    }];
    for i in 0..set.segments.len() {
        let next = if i + 1 >= set.gates.len() {
            None
        } else if flat {
            // one cell: just the gate, soonest
            Some(Arc::new(ValueTable {
                gate: set.gates[i + 1].gate(),
                nodes: std::array::from_fn(|_| vec![0.0]),
                values: vec![0.0],
            }))
        } else {
            let done = load(&out.join(format!("seg_{}.json", i + 1)))?.ok_or("run tables first")?;
            Some(Arc::new(table_of(&set.gates[i + 1], &done)))
        };
        let st = stretch(world, course, set, i, next)?;
        let classes = if flat {
            Vec::new()
        } else {
            load(&out.join(format!("seg_{i}.json")))?
                .ok_or("run tables first")?
                .classes
        };
        let t0 = Instant::now();
        let mut found: Vec<(String, f64, Partial)> = beam
            .par_iter()
            .flat_map_iter(|b| {
                let s = *b.x.last().unwrap();
                let flown = b.u.len() as f64;
                // the table's seeds and every route's
                let mut got: Vec<(String, f64, Plan)> = classes
                    .par_iter()
                    .filter_map(|c| solve_from(&st, s, &c.u).map(|(v, p)| (c.label.clone(), v, p)))
                    .collect();
                got.extend(
                    all_routes(&st, s)
                        .into_iter()
                        .filter_map(|(l, b, _, _)| b.map(|(v, p)| (l, v, p))),
                );
                let got = distinct(got, BRANCH);
                got.into_iter()
                    .map(|(l, v, p)| {
                        let mut n = b.clone();
                        n.u.extend_from_slice(&p.u);
                        n.x.extend_from_slice(&p.x[1..]);
                        n.labels.push(l.clone());
                        n.predicted = flown + v;
                        (l, n.predicted, n)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        found.sort_by(|a, b| a.1.total_cmp(&b.1));
        // no route takes the whole beam. why? a table's read is too rough to rank by (a stitched flight
        // polishes about 100 ticks shorter, by unequal amounts), so what's kept is variety for `optimize`
        let mut per_route = std::collections::HashMap::<String, usize>::new();
        found.retain(|f| {
            let n = per_route.entry(f.0.clone()).or_default();
            *n += 1;
            *n <= PER_ROUTE
        });
        found.truncate(BEAM);
        let seg = &set.segments[i];
        println!(
            "{} -> {} ({:.0} s):",
            seg.from,
            seg.to,
            t0.elapsed().as_secs_f64()
        );
        for f in &found {
            println!(
                "  predicted {:.2}, at tick {}, routes {:?}",
                f.1,
                f.2.u.len(),
                f.2.labels
            );
        }
        if found.is_empty() {
            return Err(format!("{}: nothing got through", seg.from));
        }
        beam = found.into_iter().map(|f| f.2).collect();
    }
    let dir_h = dir.join("runs/hybrid");
    std::fs::create_dir_all(&dir_h).map_err(|e| e.to_string())?;
    for (k, b) in beam.iter().enumerate() {
        let head: Vec<_> = rec.rows[..exit].to_vec();
        let mut rows = head;
        let proto = rec.rows[exit].clone();
        let mut yaw = proto.input.yaw;
        for (t, u) in b.u.iter().enumerate() {
            let (input, y) = etas::solve::ddp::input_at(&b.x[t], u, yaw);
            yaw = y;
            let mut r = proto.clone();
            r.t = exit + t + 1;
            r.input.yaw = input.yaw as f64;
            r.input.pitch = input.pitch as f64;
            let n = b.x[t + 1];
            r.state.position = [n[0], n[1], n[2]];
            r.state.delta_movement = [n[3], n[4], n[5]];
            rows.push(r);
        }
        // coasting past the last state, since `optimize --hybrid` reads a finish from a row after it
        let last = rows.last().unwrap().clone();
        for j in 1..=COAST {
            let mut r = last.clone();
            r.t += j;
            let v = last.state.delta_movement;
            r.state.position = std::array::from_fn(|m| last.state.position[m] + v[m] * j as f64);
            rows.push(r);
        }
        let mut h = rec.header.clone();
        h.source = format!("routes {name} {:?}", b.labels);
        let traj = Trajectory {
            header: h,
            initial: rec.initial.clone(),
            arrivals: Vec::new(),
            rows,
        };
        traj.write(&dir_h.join(format!("{tag}_{k}.traj.jsonl")))?;
        println!("{tag}_{k}: {:.2} timed, {:?}", b.predicted, b.labels);
    }
    Ok(())
}

#[derive(Deserialize)]
struct StartJ {
    label: String,
    class: String,
    kind: String,
    x: X,
    path: Vec<[f64; 3]>,
    touches: usize,
}

#[derive(Serialize)]
struct StartOut {
    label: String,
    class: String,
    kind: String,
    touches: usize,
    x: X,
    /// Ticks from the exit to the finish, as the exit's table reads, and ticks to the exit.
    value: f64,
    ticks: usize,
    u: Vec<U>,
    exit: X,
}

/// `runs/routes/starts.json`'s states against the first gate's table, time before it free: what
/// each way of starting comes to once its flight down is optimized, not as it was flown.
fn start(dir: &Path, name: &str, set: &SetJ, world: &World) -> Result<(), String> {
    let text =
        std::fs::read_to_string(dir.join("runs/routes/starts.json")).map_err(|e| e.to_string())?;
    let states: Vec<StartJ> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let done =
        load(&dir.join(format!("runs/routes/{name}/seg_0.json")))?.ok_or("run tables first")?;
    let table = Arc::new(table_of(&set.gates[0], &done));
    let pts: Vec<[f64; 3]> = states.iter().flat_map(|s| s.path.iter().copied()).collect();
    let walls = Walls::around(world, &pts, 12.0, 1.0);
    let results: Vec<StartOut> = states
        .par_iter()
        .filter_map(|c| {
            let cap = c.path.len() + 60;
            let pb = Problem {
                kernel: Generic::default(),
                walls: &walls,
                x0: c.x,
                goal: Goal::Table {
                    table: table.clone(),
                    cap,
                    time_weight: 0.0,
                },
                opts: opts(),
            };
            let mut best: Option<(f64, Plan)> = None;
            for ahead in AHEADS {
                for cross in CROSSES {
                    let seed = guided(&walls, &c.x, &c.path, ahead, cross, cap);
                    let s = pb.solve(&seed);
                    if good(&pb, &s.plan) {
                        let v = pb.value(&s.plan.x);
                        if best.as_ref().is_none_or(|b| v < b.0) {
                            best = Some((v, s.plan));
                        }
                    }
                }
            }
            let (value, plan) = best?;
            Some(StartOut {
                label: c.label.clone(),
                class: c.class.clone(),
                kind: c.kind.clone(),
                touches: c.touches,
                x: c.x,
                value,
                ticks: plan.u.len(),
                exit: *plan.x.last().unwrap(),
                u: plan.u,
            })
        })
        .collect();
    let mut by_value: Vec<&StartOut> = results.iter().collect();
    by_value.sort_by(|a, b| a.value.total_cmp(&b.value));
    println!("{} of {} states solved", results.len(), states.len());
    for r in by_value.iter().take(40) {
        let e = r.exit;
        println!(
            "{:8.2}  {:<46} y {:6.1} z {:6.1} speed {:4.2} in {:3} ticks, exit v {:5.2} {:5.2} {:5.2}",
            r.value,
            r.label.chars().take(46).collect::<String>(),
            r.x[1],
            r.x[2],
            (r.x[3] * r.x[3] + r.x[4] * r.x[4] + r.x[5] * r.x[5]).sqrt(),
            r.ticks,
            e[3],
            e[4],
            e[5],
        );
    }
    std::fs::write(
        dir.join(format!("runs/routes/{name}/starts.json")),
        serde_json::to_string(&results).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn run() -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().ok_or("want a set")?;
    let phase = args.get(1).ok_or("want tables or forward")?;
    let text = std::fs::read_to_string(dir.join(format!("runs/routes/{name}.json")))
        .map_err(|e| e.to_string())?;
    let set: SetJ = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let (course, world) = Course::load(dir)?;
    match phase.as_str() {
        "tables" => tables(dir, name, &set, &course, &world),
        "forward" | "flat" => {
            let f = args.get(2).map_or("relax/fullrange2", |s| s.as_str());
            forward(dir, name, &set, &course, &world, f, phase == "flat")
        }
        "start" => start(dir, name, &set, &world),
        p => Err(format!("{p}?")),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("routes: {e}");
            ExitCode::FAILURE
        }
    }
}
