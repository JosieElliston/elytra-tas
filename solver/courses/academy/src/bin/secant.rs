//! Phase `secant`: a line made of chords, and whether it carries back to a better L.
//!
//! A tick's move goes along y, then the larger horizontal axis, then the smaller, so its corners land somewhere new whenever
//! the ticks slide along the line: a flight clear at one sub-tick phase can hit at another. A chord barely moves when they
//! slide, so a line clear by chords is nearly continuous in time. `read` prices BASE's contacts by what would clear them.
//! `pure` walks BASE out to chords (`xz` keeps the y leg, `xyz` is straight on all three axes; no tick may fall back on its
//! L), each step warm from the last and undone if it collapses, then carries the line back to the L two ways: `homotopy`
//! slides every tick's corners back together, `prefix` turns ticks back into L's from the start. `phase` lines read how each
//! collision model takes the plan's positions resampled at sub-tick phases. Only a plan at `s = 0` flies.
//!
//! `cargo run --release -p academy --bin secant -- read BASE MARGIN`
//! `cargo run --release -p academy --bin secant -- pure BASE MARGIN xz|xyz`

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use etas::course::Course;
use etas::solve::bounces::{flight, state_at};
use etas::solve::ddp::{Goal, Opts, Plan as DPlan, Problem, Solved, U, X, control, lead_of};
use etas::solve::walls::Walls;
use etas::trajectory::Trajectory;
use sim::generic::Generic;
use sim::generic::ghost::{l_move, relaxed_move};

const XZ: [f64; 2] = [1.0, 0.0];
const XYZ: [f64; 2] = [1.0, 1.0];

struct Setup {
    a: usize,
    x0: X,
    seed: Vec<U>,
    goal: Goal,
    walls: Walls,
}

fn setup(dir: &Path, base: &str) -> Result<Setup, String> {
    let (course, world) = Course::load(dir)?;
    let rec = Trajectory::read(&dir.join(format!("runs/relax/{base}.traj.jsonl")))?;
    let start = &course.region("start_area")?.working;
    let finish = &course.region("finish")?.working;
    let at = finish.min[2].ok_or("finish has no z_min")?;
    let pos = |t: usize| state_at(&rec, t).position;
    let n = rec.rows.len();
    let timer = start.exit_move((0..n).map(pos)).ok_or("no timer")?;
    let f = (timer..n).find(|&t| pos(t - 1)[2] <= at && pos(t)[2] > at).ok_or("no finish")?;
    let a = timer - 1;
    let pts: Vec<[f64; 3]> = rec.rows.iter().map(|x| x.state.position).collect();
    let seed = (a..f)
        .map(|t| {
            let i = &rec.rows[t].input;
            control(lead_of(&flight(state_at(&rec, t)), i.yaw), i.pitch)
        })
        .collect();
    let goal = Goal::Gate {
        normal: [0.0, 0.0, 1.0],
        at,
        cap: f - a + 80,
        lo: [finish.min[0].unwrap_or(f64::NEG_INFINITY), finish.min[1].unwrap_or(f64::NEG_INFINITY), f64::NEG_INFINITY],
        hi: finish.max.map(|m| m.unwrap_or(f64::INFINITY)),
    };
    Ok(Setup { a, x0: flight(state_at(&rec, a)), seed, goal, walls: Walls::around(&world, &pts, 6.0, 1.0) })
}

fn problem<'a>(s: &'a Setup, margin: f64, relax: Option<Vec<[f64; 2]>>) -> Problem<'a> {
    Problem {
        kernel: Generic::default(),
        walls: &s.walls,
        x0: s.x0,
        goal: s.goal.clone(),
        opts: Opts { tol: 1e-6, margin, server: true, c0: 1e3, iters: 300, relax: relax.map(Arc::new), ..Opts::default() },
    }
}

/// A plan's reading under the L: value, least clearance, and how many ticks are under the margin.
fn on_l(s: &Setup, margin: f64, p: &DPlan) -> (f64, f64, usize) {
    let pb = problem(s, margin, None);
    let n = p.u.len();
    let under = (0..n).filter(|&k| pb.worst(&p.x[k..=k + 1], &p.u[k..k + 1]) < margin - 0.002).count();
    (pb.value(&p.x), pb.worst(&p.x, &p.u), under)
}

fn report(s: &Setup, margin: f64, name: &str, pb: &Problem, r: &Solved, secs: f64) {
    let iters: usize = r.stages.iter().map(|x| x.iters).sum();
    let (v, w, under) = on_l(s, margin, &r.plan);
    println!(
        "{name:24} value {:.4} clear {:.4} feasible {} | {secs:.1} s, {iters} iters, {} stages | as an L: value {v:.4} clear {w:.4}, {under} ticks under",
        pb.value(&r.plan.x),
        pb.worst(&r.plan.x, &r.plan.u),
        r.feasible,
        r.stages.len(),
    );
}

/// The clearance of a polyline.
fn path_clear(w: &Walls, c: &[[f64; 3]]) -> f64 {
    c.windows(2).map(|p| w.segment_clearance(p[0], p[1]).0).fold(f64::INFINITY, f64::min)
}

/// Per kind of leg (the move, the server's copy, its copy a tick behind) and per tick: where the legs start, by what.
fn origins(x: &[X], k: usize) -> [([f64; 3], [f64; 3]); 3] {
    let (p, q) = (x[k], x[k + 1]);
    let (pp, pq) = ([p[0], p[1], p[2]], [q[0], q[1], q[2]]);
    [(pp, [q[3], q[4], q[5]]), (pq, [q[3], q[4], q[5]]), (pq, [p[3], p[4], p[5]])]
}

fn read(s: &Setup, margin: f64) {
    let pb = problem(s, margin, None);
    let t0 = Instant::now();
    let r = pb.solve(&s.seed);
    report(s, margin, "L", &pb, &r, t0.elapsed().as_secs_f64());
    let (x, n) = (&r.plan.x, r.plan.u.len());
    let total: f64 = r.mult.walls.iter().flatten().sum();
    println!("total price {total:.3} ticks per block over {n} ticks");
    // price per leg: y, first horizontal, second, by which copy; and by what would clear it
    let mut by_leg = [[0.0; 3]; 3];
    let mut by_fix = std::collections::BTreeMap::<&str, f64>::new();
    let mut rows = Vec::new();
    // ticks whose L is clear but whose chord isn't: where the relaxation is no relaxation
    let (mut chord_hits, mut chord3_hits) = (0, 0);
    for k in 0..n {
        for (copy, (p, v)) in origins(x, k).into_iter().enumerate() {
            if copy > 0 && k + 1 == n {
                continue;
            }
            let [a, b, e] = l_move(p, v);
            let l = path_clear(&s.walls, &[p, a, b, e]);
            let xz = path_clear(&s.walls, &[p, a, e]);
            let xyz = path_clear(&s.walls, &[p, e]);
            let other = [a[0] + e[0] - b[0], a[1], a[2] + e[2] - b[2]];
            let swap = path_clear(&s.walls, &[p, a, other, e]);
            if l >= margin - 0.002 && xz < margin - 0.002 {
                chord_hits += 1;
            }
            if l >= margin - 0.002 && xyz < margin - 0.002 {
                chord3_hits += 1;
            }
            let m = &r.mult.walls[k][3 * copy..3 * copy + 3];
            let price: f64 = m.iter().sum();
            if price < 1e-3 {
                continue;
            }
            for i in 0..3 {
                by_leg[copy][i] += m[i];
            }
            // by more than a tie: a chord shares the L's ends, so a contact at the tick's own pose reads the margin exactly
            let eased = margin + 1e-3;
            let fix = if xz >= eased {
                "the xz chord clears"
            } else if xyz >= eased {
                "only the 3-axis chord clears"
            } else if swap >= eased {
                "only the other L order clears"
            } else {
                "nothing straighter clears"
            };
            *by_fix.entry(fix).or_default() += price;
            rows.push((k, copy, price, *m.iter().max_by(|p, q| p.total_cmp(q)).unwrap(), m.iter().position(|y| *y == m.iter().copied().fold(0.0, f64::max)).unwrap(), fix, xz, xyz, swap, v, p));
        }
    }
    println!("price by copy (move, server, behind) and leg (y, first, second): {by_leg:.3?}");
    println!("price by what would clear it, at margin {margin}:");
    for (f, v) in &by_fix {
        println!("  {f:32} {v:8.3} ({:.0}%)", 100.0 * v / total);
    }
    println!("legs clear as an L and not as a chord: xz {chord_hits}, 3-axis {chord3_hits}");
    println!("tick copy leg price  xz-clear 3ax-clear other  |vx|/|vz| pos");
    for (k, copy, price, _, leg, fix, xz, xyz, swap, v, p) in rows {
        println!(
            "t{:<4} {} {} {price:7.3} {xz:7.3} {xyz:7.3} {swap:7.3} {:6.2} [{:.2} {:.2} {:.2}] {fix}",
            k + s.a,
            ["m", "s", "b"][copy],
            ["y", "1", "2"][leg],
            v[0].abs() / v[2].abs().max(1e-9),
            p[0],
            p[1],
            p[2],
        );
    }
}

/// How each collision model takes the plan's positions resampled at sub-tick phases: per model, the least clearance over
/// the phases and the most ticks under 0 at one. Move legs only.
fn phase(s: &Setup, name: &str, x: &[X]) {
    let mut out = String::new();
    for (model, w) in [("L", [0.0; 2]), ("xz", XZ), ("xyz", XYZ)] {
        let (mut least, mut most) = (f64::INFINITY, 0);
        for d in 1..10 {
            let d = d as f64 / 10.0;
            let at = |k: usize| -> [f64; 3] { std::array::from_fn(|i| x[k][i] + (x[k + 1][i] - x[k][i]) * d) };
            let mut hit = 0;
            for k in 0..x.len() - 2 {
                let (p, q) = (at(k), at(k + 1));
                let v = std::array::from_fn(|i| q[i] - p[i]);
                let [a, b, e] = relaxed_move::<f64>(p, v, w);
                let c = path_clear(&s.walls, &[p, a, b, e]);
                least = least.min(c);
                hit += (c < 0.0) as usize;
            }
            most = most.max(hit);
        }
        out += &format!(" | {model}: least {least:.4}, up to {most} ticks hit");
    }
    println!("phase {name}{out}");
}

const TIGHT: f64 = 0.002;
const COLLAPSE: f64 = 3.0;

/// From `start` along `path` over `f0..f1`, each solve warm from the last. A step that loses the finish, the margin or
/// `COLLAPSE` ticks is undone and halved. The plan and how far it got.
fn walk(s: &Setup, margin: f64, name: &str, start: &Solved, path: impl Fn(f64) -> Vec<[f64; 2]>, f0: f64, f1: f64) -> (Solved, f64) {
    let (mut cur, mut f, mut step) = (start.clone(), f0, 0.1);
    let t0 = Instant::now();
    let dir = (f1 - f0).signum();
    while (f1 - f) * dir > 1e-9 {
        let to = if (f1 - f - dir * step) * dir < 1e-9 { f1 } else { f + dir * step };
        let pb = problem(s, margin, Some(path(to)));
        let last = pb.value(&cur.plan.x);
        let mut m = cur.mult.clone();
        m.rho = pb.opts.c0;
        let r = pb.solve_from(m, &cur.plan.u);
        let (v, w) = (pb.value(&r.plan.x), pb.worst(&r.plan.x, &r.plan.u));
        let n = r.plan.u.len();
        let ok = n > 0 && pb.crossed(&r.plan.x[n - 1], &r.plan.x[n]) && w >= margin - TIGHT && v < last + COLLAPSE;
        println!("{name} f={to:.4}: value {v:.4} clear {w:.4} {} at {:.0} s", if ok { "took" } else { "UNDONE" }, t0.elapsed().as_secs_f64());
        if ok {
            cur = r;
            f = to;
            step *= 1.5;
        } else {
            step /= 2.0;
            if step < 0.002 {
                println!("{name}: stuck at f={f:.4}");
                break;
            }
        }
    }
    (cur, f)
}

fn pure(s: &Setup, dir: &Path, base: &str, margin: f64, which: &str) -> Result<(), String> {
    let w = match which {
        "xz" => XZ,
        "xyz" => XYZ,
        o => return Err(format!("unknown model {o}")),
    };
    let pb = problem(s, margin, None);
    let t0 = Instant::now();
    let l = pb.solve(&s.seed);
    report(s, margin, "L", &pb, &l, t0.elapsed().as_secs_f64());
    phase(s, "L", &l.plan.x);
    let lv = pb.value(&l.plan.x);
    let (out, reached) = walk(s, margin, "out", &l, |f| vec![[w[0] * f, w[1] * f]], 0.0, 1.0);
    let pr = problem(s, margin, Some(vec![[w[0] * reached, w[1] * reached]]));
    report(s, margin, &format!("{which} at f={reached:.3}"), &pr, &out, t0.elapsed().as_secs_f64());
    phase(s, which, &out.plan.x);
    let d = dir.join("runs/secant");
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let rows: Vec<String> = out.plan.u.iter().map(|u| format!("{}\t{}", u[0], u[1])).collect();
    std::fs::write(d.join(format!("{base}_{which}.u.tsv")), rows.join("\n")).map_err(|e| e.to_string())?;
    let ((h, _), (p, _)) = rayon::join(
        || walk(s, margin, "homotopy", &out, |f| vec![[w[0] * f, w[1] * f]], reached, 0.0),
        || {
            let n = out.plan.u.len();
            walk(s, margin, "prefix", &out, |f| {
                (0..n + 200).map(|k| if (k as f64) < f * n as f64 { [0.0; 2] } else { [w[0] * reached, w[1] * reached] }).collect()
            }, 0.0, 1.0)
        },
    );
    for (name, r) in [("homotopy", &h), ("prefix", &p)] {
        let (v, c, under) = on_l(s, margin, &r.plan);
        println!("back by {name}: as an L value {v:.4} clear {c:.4}, {under} under (L was {lv:.4})");
        phase(s, name, &r.plan.x);
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cmd), Some(base), Some(margin)) = (args.first(), args.get(1), args.get(2)) else {
        return Err("read BASE MARGIN | pure BASE MARGIN xz|xyz".into());
    };
    let margin: f64 = margin.parse().map_err(|_| "margin")?;
    let s = setup(dir, base)?;
    match cmd.as_str() {
        "read" => read(&s, margin),
        "pure" => pure(&s, dir, base, margin, args.get(3).ok_or("xz|xyz")?)?,
        o => return Err(format!("unknown {o}")),
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("secant: {e}");
        std::process::exit(1);
    }
}
