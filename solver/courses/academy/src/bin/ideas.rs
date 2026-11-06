//! Phase `ideas`: one-off checks on a flight, for trying a suggested change to its route.
//!
//! - `blocks X0 Y0 Z0 X1 Y1 Z1`: the non-air blocks in a box and their collision boxes.
//! - `clear FLIGHT [BELOW]`: each stretch where a flight in `runs/relax` is closer than `BELOW` (0.05) to an
//!   obstacle: the ticks, the least clearance, and the box. "Close" is the gliding box swept along the move's L.
//! - `setup BASE`: BASE's way off the slab with other keys and yaws, flown under port, each against BASE at the
//!   deploy and read on `dome`'s table at the door; writes `runs/hybrid/idea_setup_NAME`.
//! - `setupgrid BASE`: the same over the timings, for the keys that change the state: S south, no strafe north.
//! - `door BASE`: from BASE's deploy to the door, the pitch as a line through K knots, searched under port on the
//!   table, for how many reversals the door needs; writes `runs/hybrid/idea_door_K`.
//! - `graft A B OUT`: A's way off the slab up to its deploy, then B's inputs from B's deploy on, flown whole under
//!   port. Written to `runs/relax/OUT` if it holds as flown, and as a seed `runs/hybrid/OUT` from the door on anyway.
//! - `finish BASE T0 OUT WEIGHT`: BASE from tick T0 to the finish as one solve, rotation priced at WEIGHT, flown under
//!   the faithful kernel by `follow`, with what that kernel's steps differ by put back into the solve. No windows, so no
//!   seams. Writes `runs/relax/OUT`.
//! - `blend A B T0 T1 OUT`: A, then between ticks T0 and T1 a smooth fade to B, then B, as `runs/hybrid/OUT`.
//! - `bump BASE T0 T1 DX DY DZ OUT`: BASE's positions moved by a smooth bump that peaks halfway, as `runs/hybrid/OUT`.
//! - `calm BASE T0 T1 OUT [WEIGHT...]`: re-fly BASE's ticks T0..T1 with rotation priced at each weight, pinned to
//!   BASE's state at T1 (0 for T1: free to the finish, so time pays for rotation), the rest kept, then flown under
//!   port and judged by `verify`, for whether a snap there buys anything. Each is `runs/relax/OUT_WEIGHT`, and
//!   `runs/relax/OUT_base` is BASE as flown. From the tick after the deploy to the finish (`calm BASE 60 0 OUT 0.1`) it's
//!   a finishing pass over the whole run, with no joins in it.
//!
//! `optimize --hybrid` then polishes these.
//!
//! `cargo run --release -p academy --bin ideas -- PHASE`

use std::path::Path;
use std::sync::Arc;

use etas::course::Course;
use etas::solve::bounces::{SCALE, flight, fly, state_at as state_at_full};
use etas::solve::ddp::{Goal, Opts, Plan as DPlan, Problem, Rate, X, control, input_at, lead_of, rotation};
use etas::solve::refly::{follow, track};
use etas::solve::smooth::DELTA;
use etas::solve::gate::{Gate, ValueTable};
use etas::solve::walls::Walls;
use etas::trajectory::{InputRow, Row, Trajectory};
use rayon::prelude::*;
use sim::faithful::Faithful;
use sim::generic::Generic;
use sim::state::FlightState;
use world::{BlockPos, Cell, World};

/// As in `dome`: the door's plane, and the table over it.
const EXIT_Z: f64 = 205.0;
const TAU: f64 = 0.5;
const MARGIN: f64 = 0.012;

fn relax(dir: &Path, name: &str) -> Result<Trajectory, String> {
    Trajectory::read(&dir.join(format!("runs/relax/{name}.traj.jsonl")))
}

fn load_table(dir: &Path) -> Result<ValueTable, String> {
    #[derive(serde::Deserialize)]
    struct J {
        nodes: Vec<Vec<f64>>,
        values: Vec<f64>,
    }
    let j: J = serde_json::from_str(&std::fs::read_to_string(dir.join("runs/dome/table.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let n = j.nodes;
    Ok(ValueTable {
        gate: Gate { axis: 2, sign: -1.0, at: -EXIT_Z, lo: [-34.0, 195.0], hi: [-18.0, 212.0] },
        nodes: [n[0].clone(), n[1].clone(), n[2].clone(), n[3].clone(), n[4].clone(), vec![TAU]],
        values: j.values,
    })
}

/// The first row past the door, and the table's reading there; None if it hits something first.
fn door_read(table: &ValueTable, walls: &Walls, t: &Trajectory, from: usize) -> Option<(usize, f64)> {
    for i in from..t.rows.len() {
        let s = &t.rows[i].state;
        if s.horizontal_collision || s.vertical_collision || s.on_ground {
            return None;
        }
        let (p, q) = (t.rows[i - 1].state.position, s.position);
        let a = [p[0], q[1], p[2]];
        let b = if (q[0] - p[0]).abs() >= (q[2] - p[2]).abs() { [q[0], q[1], p[2]] } else { [p[0], q[1], q[2]] };
        if [(p, a), (a, b), (b, q)].iter().any(|l| walls.clearance(l.0, l.1).0 < MARGIN) {
            return None;
        }
        if q[2] < EXIT_Z {
            let x: X = flight(s);
            if x[5] >= 0.0 || !table.gate.in_window(&x) {
                return None;
            }
            let tau = (EXIT_Z - x[2]) / -x[5];
            return Some((i, table.read(&table.gate.coords(&x, tau))));
        }
    }
    None
}

/// `head` to the row past the door, then `base` from its first row past that row's z.
fn write_seed(dir: &Path, base: &Trajectory, head: &Trajectory, door: usize, name: &str) -> Result<(), String> {
    let z = head.rows[door].state.position[2];
    let join = base.rows.iter().position(|r| r.state.position[2] < z).ok_or("no join")?;
    let mut rows: Vec<Row> = head.rows[..=door].to_vec();
    rows.extend(base.rows[join..].iter().cloned());
    for (i, r) in rows.iter_mut().enumerate() {
        r.t = i + 1;
    }
    let mut h = base.header.clone();
    h.source = name.to_string();
    Trajectory { header: h, initial: head.initial.clone(), arrivals: Vec::new(), rows }.write(&dir.join(format!("runs/hybrid/{name}.traj.jsonl")))
}

fn glide_walls(w: &World, base: &Trajectory) -> Walls {
    let pts: Vec<[f64; 3]> = base.rows.iter().map(|r| r.state.position).collect();
    Walls::around(w, &pts, 8.0, 1.0)
}

fn rows_of(base: &Trajectory, door: usize) -> Vec<InputRow> {
    base.rows[..=door].iter().map(|r| r.input.clone()).collect()
}

/// How a head's rotations add up: the sum of each tick's turn, the short way, and how often the pitch changes direction.
fn turning(i: &[InputRow]) -> (f64, usize) {
    let (mut yaw, mut rev, mut last) = (0.0, 0, 0.0f64);
    for w in i.windows(2) {
        yaw += ((w[1].yaw - w[0].yaw + 180.0).rem_euclid(360.0) - 180.0).abs();
        let d = w[1].pitch - w[0].pitch;
        if d != 0.0 {
            if d * last < 0.0 {
                rev += 1;
            }
            last = d;
        }
    }
    (yaw, rev)
}

fn head_line(name: &str, base: &Trajectory, deploy: usize, f: &Trajectory, read: Option<(usize, f64)>, inputs: &[InputRow]) {
    let (a, b) = (&base.rows[deploy].state, f.rows.get(deploy).map(|r| &r.state));
    let dv = b.map_or(f64::NAN, |b| (0..3).map(|k| (a.delta_movement[k] - b.delta_movement[k]).powi(2)).sum::<f64>().sqrt());
    let dp = b.map_or(f64::NAN, |b| (0..3).map(|k| (a.position[k] - b.position[k]).powi(2)).sum::<f64>().sqrt());
    let (yaw, rev) = turning(inputs);
    println!("{name:16} at base's deploy: off {dp:.6} blocks, {dv:.6} b/t; yaw turned {yaw:5.0}, pitch reversals {rev:2}; door read {}", read.map_or("hits".into(), |r| format!("{:.3} (row {})", r.1, r.0 + 1)));
}

/// Replace rows `from..=to` (1-based) of `v` with a yaw and keys.
fn set(v: &mut [InputRow], from: usize, to: usize, yaw: f64, keys: &str) {
    for r in &mut v[from - 1..to] {
        r.yaw = yaw;
        r.keys = keys.to_string();
    }
}

/// Rows `from..=to`: no keys, the yaw going from `a` to `b` the short way, since with no keys the yaw does nothing in the air.
fn ramp(v: &mut [InputRow], from: usize, to: usize, a: f64, b: f64) {
    let d = (b - a + 180.0).rem_euclid(360.0) - 180.0;
    for (i, r) in v[from - 1..to].iter_mut().enumerate() {
        r.yaw = a + d * (i + 1) as f64 / (to - from + 2) as f64;
        r.keys = "-".to_string();
    }
}

fn setup(dir: &Path, w: &World, base_name: &str) -> Result<(), String> {
    let base = relax(dir, base_name)?;
    let table = load_table(dir)?;
    let walls = glide_walls(w, &base);
    let deploy = base.rows.iter().position(|r| r.state.fall_flying).ok_or("no deploy")?;
    let door = base.rows.iter().position(|r| r.state.position[2] < EXIT_Z).ok_or("no door")?;
    let air = base.rows.iter().position(|r| r.input.keys.contains('J')).ok_or("no jump")?;
    // ticks by the base's: south on a strafe 12..=23, no keys 24..=31, north on a strafe 32..=58, deploy 59
    let south = base.rows.iter().position(|r| r.input.yaw > 300.0).ok_or("no south")? + 1;
    let south_end = base.rows[south..].iter().position(|r| r.input.yaw < 300.0).ok_or("no end")? + south;
    let north = base.rows[south_end..].iter().position(|r| r.input.keys.contains('W')).ok_or("no north")? + south_end + 1;
    println!("base: jump row {} south {south}..={south_end} north {north}..={deploy} deploy {} door {}", air + 1, deploy + 1, door + 1);
    let y0 = base.rows[0].input.yaw;
    let yn = base.rows[north - 1].input.yaw;
    let y_s = y0 + 225.0;
    let mut variants: Vec<(&str, Vec<InputRow>)> = vec![("base", rows_of(&base, door))];
    // the coast's look back does nothing
    let mut v = rows_of(&base, door);
    ramp(&mut v, south_end + 1, north - 1, y_s, yn);
    variants.push(("no_look_back", v));
    // south on the other strafe
    let mut v = rows_of(&base, door);
    set(&mut v, south, south_end, y0 + 135.0, "WDR");
    ramp(&mut v, south_end + 1, north - 1, y0 + 135.0, yn);
    variants.push(("south_d", v));
    // north on the other strafe
    let mut v = rows_of(&base, door);
    set(&mut v, south, south_end, y_s - 360.0, "WAR");
    set(&mut v, north, deploy, yn - 90.0, "WDR");
    ramp(&mut v, south_end + 1, north - 1, y_s - 360.0, yn - 90.0);
    variants.push(("north_d", v));
    // north with no strafe
    let mut v = rows_of(&base, door);
    set(&mut v, south, south_end, y0 + 135.0, "WDR");
    set(&mut v, north, deploy, yn - 45.0, "WR");
    ramp(&mut v, south_end + 1, north - 1, y0 + 135.0, yn - 45.0);
    variants.push(("south_d_north_w", v));
    // south on S, no turn
    let mut v = rows_of(&base, door);
    set(&mut v, south, south_end, y0, "S");
    ramp(&mut v, south_end + 1, north - 1, y0, yn);
    variants.push(("south_s", v));
    for (name, inputs) in &variants {
        let (f, stopped) = fly(&base.initial, inputs, &[], w, base.header.clone());
        if stopped.is_some() {
            println!("{name}: port stopped {stopped:?}");
            continue;
        }
        let read = door_read(&table, &walls, &f, deploy + 1);
        head_line(name, &base, deploy, &f, read, inputs);
        if let (Some((d, _)), true) = (read, *name != "base") {
            write_seed(dir, &base, &f, d, &format!("idea_setup_{name}"))?;
        }
    }
    Ok(())
}

/// Over the timings: brake rows, the sprint north's start and the deploy, keys given.
fn setupgrid(dir: &Path, w: &World, base_name: &str) -> Result<(), String> {
    let base = relax(dir, base_name)?;
    let table = load_table(dir)?;
    let walls = glide_walls(w, &base);
    let deploy = base.rows.iter().position(|r| r.state.fall_flying).ok_or("no deploy")?;
    let door = base.rows.iter().position(|r| r.state.position[2] < EXIT_Z).ok_or("no door")?;
    let air = base.rows.iter().position(|r| r.input.keys.contains('J')).ok_or("no jump")?;
    let y0 = base.rows[0].input.yaw;
    let tail: Vec<InputRow> = base.rows[deploy..=door + 2].to_vec().iter().map(|r| r.input.clone()).collect();
    #[derive(Clone, Copy, Debug)]
    struct G {
        hold: usize,
        brake: usize,
        coast: usize,
        deploy: usize,
        back: u8,
        north: u8,
    }
    let inputs_of = |g: G| -> Vec<InputRow> {
        let mut v: Vec<InputRow> = base.rows[..=air].iter().map(|r| r.input.clone()).collect();
        let row = |yaw: f64, k: &str| InputRow { yaw, pitch: 0.0, keys: k.to_string() };
        // what the north sprint looks like, for a coast with the yaw on its way
        let (yn, kn) = match g.north {
            0 => (y0 + 45.0, "WAR"),
            1 => (y0, "WR"),
            _ => (y0 + 3.0, "WR"),
        };
        for a in 1..g.deploy {
            v.push(if a >= g.coast {
                row(yn, kn)
            } else if a <= g.hold {
                row(y0, "WR")
            } else if a <= g.hold + g.brake {
                match g.back {
                    0 => row(y0, "S"),
                    1 => row(y0 + 180.0, "WR"),
                    _ => row(y0 + 135.0, "WDR"),
                }
            } else {
                row(yn, "-")
            });
        }
        v.extend(tail.iter().cloned());
        v
    };
    let mut jobs = Vec::new();
    for back in [0u8, 2] {
        for north in [0u8, 1, 2] {
            for hold in 0..=4 {
                for brake in (4..=30).step_by(2) {
                    for coast in (hold + brake + 1)..=40 {
                        for deploy in 44..=56 {
                            jobs.push(G { hold, brake, coast, deploy, back, north });
                        }
                    }
                }
            }
        }
    }
    let scored: Vec<(f64, usize, G, Vec<InputRow>, Trajectory)> = jobs
        .par_iter()
        .filter_map(|&g| {
            let inputs = inputs_of(g);
            let (f, stopped) = fly(&base.initial, &inputs, &[], w, base.header.clone());
            if stopped.is_some() {
                return None;
            }
            let (d, v) = door_read(&table, &walls, &f, air + 2 + g.deploy - 1)?;
            Some((v, d, g, inputs, f))
        })
        .collect();
    let (f, _) = fly(&base.initial, &rows_of(&base, door), &[], w, base.header.clone());
    println!("base door read {:.3}; {} of {} timings reach the door", door_read(&table, &walls, &f, deploy + 1).ok_or("base")?.1, scored.len(), jobs.len());
    for back in [0u8, 2] {
        for north in [0u8, 1, 2] {
            let mut of: Vec<_> = scored.iter().filter(|s| s.2.back == back && s.2.north == north).collect();
            of.sort_by(|a, b| a.0.total_cmp(&b.0));
            if let Some(b) = of.first() {
                let (yaw, rev) = turning(&b.3[..=b.1]);
                println!("south {} north {}: read {:.3} {:?}, yaw turned {yaw:.0}, reversals {rev}", ["S", "W", "strafe"][back as usize], ["strafe", "W", "W+3"][north as usize], b.0, b.2);
                for (k, c) in of.iter().take(2).enumerate() {
                    write_seed(dir, &base, &c.4, c.1, &format!("idea_grid_{}{}_{k}", ["s", "w", "d"][back as usize], ["a", "w", "t"][north as usize]))?;
                }
            }
        }
    }
    Ok(())
}

/// From the deploy to the door, per number of knots.
fn door(dir: &Path, w: &World, base_name: &str) -> Result<(), String> {
    let base = relax(dir, base_name)?;
    let table = load_table(dir)?;
    let walls = glide_walls(w, &base);
    let deploy = base.rows.iter().position(|r| r.state.fall_flying).ok_or("no deploy")?;
    let d0 = base.rows.iter().position(|r| r.state.position[2] < EXIT_Z).ok_or("no door")?;
    let (f, _) = fly(&base.initial, &rows_of(&base, d0), &[], w, base.header.clone());
    println!("base: door read {:.3}, deploy row {} door row {}, pitches {:?}", door_read(&table, &walls, &f, deploy + 1).ok_or("base")?.1, deploy + 1, d0 + 1, base.rows[deploy..=d0].iter().map(|r| (r.input.pitch * 10.0).round() / 10.0).collect::<Vec<_>>());
    let n = d0 - deploy;
    let yaws: Vec<f64> = base.rows[deploy + 1..=d0 + 3].iter().map(|r| r.input.yaw).collect();
    let fly_knots = |k: &[f64]| -> Option<(f64, usize, Vec<InputRow>, Trajectory)> {
        let mut inputs: Vec<InputRow> = base.rows[..=deploy].iter().map(|r| r.input.clone()).collect();
        for (i, yaw) in yaws.iter().enumerate() {
            let u = (i as f64 / n as f64 * (k.len() - 1) as f64).min((k.len() - 1) as f64 - 1e-9);
            let (j, a) = (u as usize, u.fract());
            inputs.push(InputRow { yaw: *yaw, pitch: k[j] + (k[j + 1] - k[j]) * a, keys: "WR".to_string() });
        }
        let (f, stopped) = fly(&base.initial, &inputs, &[], w, base.header.clone());
        if stopped.is_some() {
            return None;
        }
        let (d, v) = door_read(&table, &walls, &f, deploy + 1)?;
        Some((v, d, inputs, f))
    };
    for knots in [2usize, 3, 4, 5, 6, 8] {
        // a seeded search: cross entropy over the knots
        let mut mean: Vec<f64> = (0..knots).map(|i| base.rows[deploy + 1 + i * (n - 1) / (knots - 1)].input.pitch).collect();
        let mut sd = vec![15.0; knots];
        let mut best: Option<(f64, usize, Vec<InputRow>, Trajectory, Vec<f64>)> = None;
        let mut seed = 0x9E3779B97F4A7C15u64 ^ knots as u64;
        let mut draw = move || {
            // xorshift, then a sum of uniforms for a rough normal
            let mut s = 0.0;
            for _ in 0..6 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                s += (seed >> 11) as f64 / (1u64 << 53) as f64;
            }
            s - 3.0
        };
        for _ in 0..30 {
            let pop: Vec<Vec<f64>> = (0..600).map(|_| (0..knots).map(|i| (mean[i] + sd[i] * draw() / 0.7).clamp(-20.0, 85.0)).collect()).collect();
            let mut got: Vec<(f64, usize, Vec<InputRow>, Trajectory, Vec<f64>)> = pop.par_iter().filter_map(|k| fly_knots(k).map(|(v, d, i, f)| (v, d, i, f, k.clone()))).collect();
            got.sort_by(|a, b| a.0.total_cmp(&b.0));
            if got.len() < 20 {
                sd.iter_mut().for_each(|s| *s *= 1.2);
                continue;
            }
            for i in 0..knots {
                let e: Vec<f64> = got[..20].iter().map(|g| g.4[i]).collect();
                mean[i] = e.iter().sum::<f64>() / 20.0;
                sd[i] = ((e.iter().map(|x| (x - mean[i]).powi(2)).sum::<f64>() / 20.0).sqrt() * 1.1).max(0.05);
            }
            if best.as_ref().is_none_or(|b| got[0].0 < b.0) {
                best = Some(got.swap_remove(0));
            }
        }
        match best {
            Some((v, d, i, f, k)) => {
                let (_, rev) = turning(&i[deploy..=d]);
                println!("{knots} knots: door read {v:.3}, row {}, reversals {rev}, knots {:?}", d + 1, k.iter().map(|x| (x * 10.0).round() / 10.0).collect::<Vec<_>>());
                write_seed(dir, &base, &f, d, &format!("idea_door_{knots}"))?;
            }
            None => println!("{knots} knots: nothing reaches the door"),
        }
    }
    Ok(())
}

fn graft(dir: &Path, course: &Course, w: &World, a: &str, b: &str, out: &str) -> Result<(), String> {
    let (a, b) = (relax(dir, a)?, relax(dir, b)?);
    let deploy = |t: &Trajectory| t.rows.iter().position(|r| r.state.fall_flying).ok_or("no deploy".to_string());
    let (da, db) = (deploy(&a)?, deploy(&b)?);
    let mut inputs: Vec<InputRow> = a.rows[..=da].iter().map(|r| r.input.clone()).collect();
    inputs.extend(b.rows[db + 1..].iter().map(|r| r.input.clone()));
    let (f, stopped) = fly(&a.initial, &inputs, &[], w, b.header.clone());
    let r = etas::verify::verify(course, w, etas::verify::Plan::Trajectory(&f));
    let timed = r.timer_tick().zip(r.finish_tick()).map(|(s, e)| e - s);
    let (dp, dv) = (&f.rows[da].state, &b.rows[db].state);
    let off = (0..3).map(|k| (dp.position[k] - dv.position[k]).powi(2)).sum::<f64>().sqrt();
    let n = f.rows.len().min(da + 40);
    let (yaw, rev) = turning(&inputs[..n]);
    println!("{out}: off B at the deploy {off:.6}; flown whole: {} timed {timed:?}, stopped {stopped:?}; first 40 past deploy: yaw turned {yaw:.0}, pitch reversals {rev}", if r.ok() { "PASS" } else { "FAIL" });
    if r.ok() && stopped.is_none() {
        f.write(&dir.join(format!("runs/relax/{out}.traj.jsonl")))?;
    }
    let d = f.rows.iter().position(|x| x.state.position[2] < EXIT_Z).ok_or("no door")?;
    write_seed(dir, &b, &f, d, out)
}

/// The largest change of pitch and of yaw in one tick, over rows `a..b` of `inputs`, in degrees; and the pitch's travel.
fn snap(inputs: &[InputRow], a: usize, b: usize) -> (f64, f64, f64) {
    let (mut p, mut y, mut travel) = (0.0f64, 0.0f64, 0.0);
    for k in a.max(1)..b.min(inputs.len()) {
        let dp = (inputs[k].pitch - inputs[k - 1].pitch).abs();
        p = p.max(dp);
        travel += dp;
        y = y.max(((inputs[k].yaw - inputs[k - 1].yaw + 180.0).rem_euclid(360.0) - 180.0).abs());
    }
    (p, y, travel)
}

#[allow(clippy::too_many_arguments)]
fn calm(dir: &Path, course: &Course, w: &World, base_name: &str, t0: usize, t1: usize, out: &str, weights: &[f64]) -> Result<(), String> {
    let base = relax(dir, base_name)?;
    let walls = glide_walls(w, &base);
    let f = &course.region("finish")?.working;
    let at = f.min[2].ok_or("finish has no z_min")?;
    let n = base.rows.len();
    let end = (t0..n).find(|&t| state_at_full(&base, t).position[2] <= at && state_at_full(&base, t + 1).position[2] > at).ok_or("no finish")? + 1;
    let ctl = |t: usize| {
        let i = &base.rows[t].input;
        control(lead_of(&flight(state_at_full(&base, t)), i.yaw), i.pitch)
    };
    let opts = Opts { tol: 1e-6, margin: MARGIN, server: true, ..Opts::default() };
    let gate = Problem {
        kernel: Generic::default(),
        walls: &walls,
        x0: flight(state_at_full(&base, t0)),
        goal: Goal::Gate {
            normal: [0.0, 0.0, 1.0],
            at,
            cap: end - t0 + 80,
            lo: [f.min[0].unwrap_or(f64::NEG_INFINITY), f.min[1].unwrap_or(f64::NEG_INFINITY), f64::NEG_INFINITY],
            hi: f.max.map(|m| m.unwrap_or(f64::INFINITY)),
        },
        opts: opts.clone(),
    };
    let whole = DPlan { u: (t0..end).map(ctl).collect(), x: (t0..=end).map(|t| flight(state_at_full(&base, t))).collect() };
    let last = if t1 == 0 { end } else { t1 };
    let flown_inputs: Vec<InputRow> = base.rows.iter().map(|r| r.input.clone()).collect();
    let judge = |inputs: &[InputRow], name: &str| -> Result<(), String> {
        let (tr, stopped) = fly(&base.initial, inputs, &[], w, base.header.clone());
        let r = etas::verify::verify(course, w, etas::verify::Plan::Trajectory(&tr));
        let timed = r.timer_tick().zip(r.finish_tick()).map(|(s, e)| e - s);
        let (dp, dy, travel) = snap(inputs, t0, last + 1);
        if let Some(e) = r.failures.first() {
            println!("  {name}: {e}");
        }
        let z = |t: usize| state_at_full(&tr, t).position[2];
        let g = (t0.max(1)..tr.rows.len()).find(|&t| z(t - 1) <= at && z(t) > at);
        let spare = g.map_or(f64::NAN, |g| (z(g) - at) / (z(g) - z(g - 1)));
        println!(
            "{name:18} {} timed {timed:?} past the plane at row {g:?}, {spare:.3} ticks spare, secant {} stopped {stopped:?}; in the stretch: biggest dpitch {dp:.1}, dyaw {dy:.1}, pitch travel {travel:.0}",
            if r.ok() { "PASS" } else { "FAIL" },
            r.secant_seconds().map_or("-".into(), |s| format!("{:.2}", s / 0.05)),
        );
        if stopped.is_none() {
            tr.write(&dir.join(format!("runs/relax/{name}.traj.jsonl")))?;
        }
        Ok(())
    };
    judge(&flown_inputs, &format!("{out}_base"))?;
    let solved: Vec<_> = weights
        .par_iter()
        .map(|&weight| {
            let prev = (t0 > 0).then(|| rotation(&flight(state_at_full(&base, t0 - 1)), &ctl(t0 - 1)));
            let rate = |to| Rate { from: prev, to, ..Rate::new(weight, DELTA) };
            let priced: DPlan = if t1 == 0 {
                let pb = Problem { opts: Opts { rate: rate(None), ..opts.clone() }, ..gate.clone() };
                let s = pb.solve(&whole.u);
                println!("  weight {weight}: solve feasible {}, clear {:.4} (seed {:.4}), crossing at {:.3} (seed {:.3}), pitch travel {:.0} (seed {:.0})", s.feasible, pb.worst(&s.plan.x, &s.plan.u), pb.worst(&whole.x, &whole.u), t0 as f64 + pb.value(&s.plan.x), t0 as f64 + pb.value(&whole.x), pb.travel(&s.plan.x, &s.plan.u)[1], pb.travel(&whole.x, &whole.u)[1]);
                s.plan
            } else {
                let pb = Problem {
                    goal: Goal::Reach { target: whole.x[t1 - t0], scale: SCALE },
                    opts: Opts { rate: rate(Some(rotation(&whole.x[t1 - t0], &whole.u[t1 - t0]))), ..opts.clone() },
                    ..gate.clone()
                };
                let s = pb.solve(&whole.u[..t1 - t0]);
                println!("  weight {weight}: reach feasible {}, clear {:.4} (seed {:.4}), price {:.3} (seed {:.3}), pitch travel {:.0} (seed {:.0})", s.feasible, pb.worst(&s.plan.x, &s.plan.u), pb.worst(&whole.x[..=t1 - t0], &whole.u[..t1 - t0]), pb.rate_price(&s.plan.x, &s.plan.u), pb.rate_price(&whole.x[..=t1 - t0], &whole.u[..t1 - t0]), pb.travel(&s.plan.x, &s.plan.u)[1], pb.travel(&whole.x[..=t1 - t0], &whole.u[..t1 - t0])[1]);
                let mut u = s.plan.u;
                let mut x = s.plan.x;
                u.extend_from_slice(&whole.u[t1 - t0..]);
                x.extend_from_slice(&whole.x[t1 - t0 + 1..]);
                DPlan { u, x }
            };
            (weight, priced)
        })
        .collect();
    let yaw_from = state_at_full(&base, t0).y_rot;
    for (weight, plan) in solved {
        let name = format!("{out}_{weight}");
        let row = |u: &sim::state::Input<f32>| InputRow { yaw: u.yaw as f64, pitch: u.pitch as f64, keys: "WR".to_string() };
        let clean = |pb: &Problem, plan: &DPlan, yaw: f64| {
            let mut reps: Vec<_> = [24usize, 12, 6, 3].iter().map(|&k| track(w, pb, plan, yaw, k, SCALE)).collect();
            let at = reps.iter().position(|r| r.clean()).unwrap_or(0);
            reps.swap_remove(at)
        };
        // each at its own price: unpriced, the stretch gets its wobble back; priced, all of the rest gets reworked
        // why 0.01? tracked at 0.004, every wall on the run sags to it; 0.01 costs 0.02 ticks and keeps them all over 0.009
        let priced = Opts { margin: 0.01, rate: Rate::new(weight, DELTA), ..opts.clone() };
        let mut inputs: Vec<InputRow> = base.rows[..t0].iter().map(|r| r.input.clone()).collect();
        let k0 = last - t0;
        let flown = if t1 == 0 {
            clean(&Problem { opts: priced, ..gate.clone() }, &plan, yaw_from).inputs
        } else {
            // the solve's leads as they are: a re-solve puts a wobble in, and over this few ticks the kernels hardly part
            let (mut x, mut yaw, mut a) = (plan.x[0], yaw_from, Vec::new());
            for u in &plan.u[..k0] {
                let (input, y) = input_at(&x, u, yaw);
                yaw = y;
                a.push(input);
                let t = Faithful.step_input(w, &FlightState { pos: [x[0], x[1], x[2]], vel: [x[3], x[4], x[5]] }, &input).map_err(|e| format!("{e:?}"))?;
                if t.contact.is_some() {
                    return Err(format!("contact in the stretch, tick {}", t0 + a.len()));
                }
                x = [t.state.pos[0], t.state.pos[1], t.state.pos[2], t.state.vel[0], t.state.vel[1], t.state.vel[2]];
            }
            println!("  {name}: off the pin by {:.5} blocks", (0..3).map(|k| (x[k] - plan.x[k0][k]).powi(2)).sum::<f64>().sqrt());
            let tail = DPlan { u: plan.u[k0..].to_vec(), x: std::iter::once(x).chain(plan.x[k0 + 1..].iter().copied()).collect() };
            let rest = Problem { x0: x, opts: Opts { margin: 0.004, ..opts.clone() }, ..gate.clone() };
            let b = clean(&rest, &tail, yaw);
            a.into_iter().chain(b.inputs).collect()
        };
        inputs.extend(flown.iter().map(row));
        let l = *flown.last().ok_or("empty track")?;
        inputs.extend((0..7).map(|_| row(&l)));
        judge(&inputs, &name)?;
    }
    Ok(())
}

fn finish(dir: &Path, course: &Course, w: &World, base_name: &str, t0: usize, out: &str, weight: f64) -> Result<(), String> {
    let base = relax(dir, base_name)?;
    let walls = glide_walls(w, &base);
    let f = &course.region("finish")?.working;
    let at = f.min[2].ok_or("finish has no z_min")?;
    let z = |t: usize| state_at_full(&base, t).position[2];
    let end = (t0..base.rows.len()).find(|&t| z(t) <= at && z(t + 1) > at).ok_or("no finish")? + 1;
    let x0 = flight(state_at_full(&base, t0));
    let yaw_from = state_at_full(&base, t0).y_rot;
    let prev = rotation(&flight(state_at_full(&base, t0 - 1)), &{
        let i = &base.rows[t0 - 1].input;
        control(lead_of(&flight(state_at_full(&base, t0 - 1)), i.yaw), i.pitch)
    });
    let seed: Vec<_> = (t0..end).map(|t| {
        let i = &base.rows[t].input;
        control(lead_of(&flight(state_at_full(&base, t)), i.yaw), i.pitch)
    }).collect();
    let problem = |defects: Option<Arc<Vec<X>>>| Problem {
        kernel: Generic::default(),
        walls: &walls,
        x0,
        goal: Goal::Gate {
            normal: [0.0, 0.0, 1.0],
            at,
            cap: end - t0 + 80,
            lo: [f.min[0].unwrap_or(f64::NEG_INFINITY), f.min[1].unwrap_or(f64::NEG_INFINITY), f64::NEG_INFINITY],
            hi: f.max.map(|m| m.unwrap_or(f64::INFINITY)),
        },
        opts: Opts { tol: 1e-6, margin: MARGIN, server: true, rate: Rate { from: Some(prev), ..Rate::new(weight, DELTA) }, defects, ..Opts::default() },
    };
    let fs = |x: &X| FlightState { pos: [x[0], x[1], x[2]], vel: [x[3], x[4], x[5]] };
    let mut u = seed;
    let mut defects = None;
    let mut mult = None;
    for pass in 0..3 {
        let pb = problem(defects.clone());
        let s = match mult.take() {
            Some(m) if pass > 1 => pb.solve_from(m, &u),
            _ => pb.solve(&u),
        };
        // each tick's gap from the plan's own state, so drift so far doesn't count in it
        let mut near = yaw_from;
        let mut d = Vec::new();
        for (k, uk) in s.plan.u.iter().enumerate() {
            let (input, y) = input_at(&s.plan.x[k], uk, near);
            near = y;
            let g = etas::solve::ddp::step(&pb.kernel, &s.plan.x[k], *uk).0;
            d.push(match Faithful.step_input(w, &fs(&s.plan.x[k]), &input) {
                Ok(t) if t.contact.is_none() => {
                    let nx = [t.state.pos[0], t.state.pos[1], t.state.pos[2], t.state.vel[0], t.state.vel[1], t.state.vel[2]];
                    std::array::from_fn(|i| nx[i] - g[i])
                }
                _ => [0.0; 6],
            });
        }
        // then the whole plan flown, heading kept by the leads, to judge it
        let (mut x, mut yaw) = (x0, yaw_from);
        let (mut gap, mut contacts) = (0.0f64, 0);
        let mut stopped_at = None;
        for (k, uk) in s.plan.u.iter().enumerate() {
            let (input, y) = input_at(&x, uk, yaw);
            yaw = y;
            match Faithful.step_input(w, &fs(&x), &input) {
                Ok(t) => {
                    contacts += t.contact.is_some() as usize;
                    x = [t.state.pos[0], t.state.pos[1], t.state.pos[2], t.state.vel[0], t.state.vel[1], t.state.vel[2]];
                    gap = gap.max((0..3).map(|i| (x[i] - s.plan.x[k + 1][i]).powi(2)).sum::<f64>().sqrt());
                }
                Err(_) => {
                    stopped_at = Some(t0 + k);
                    break;
                }
            }
        }
        println!("  flew to {stopped_at:?}");
        let clear = pb.worst(&s.plan.x, &s.plan.u);
        println!(
            "pass {pass}: feasible {}, clear {clear:.4}, crossing {:.3}, flown off the plan by at most {gap:.5}, contacts {contacts}, ticks {}",
            s.feasible, t0 as f64 + pb.value(&s.plan.x), s.plan.u.len()
        );
        u = s.plan.u;
        mult = Some(s.mult);
        defects = Some(Arc::new(d));
    }
    let pb = problem(defects.clone());
    let plan = DPlan { x: pb.rollout(&u), u };
    // why 96? at 24 and 48 a correction still shows as a bump, 96 ticks away from any wall (academy, 2026-11-06)
    let horizon = 96;
    let rep = follow(w, &Problem { opts: Opts { margin: 0.01, ..pb.opts.clone() }, ..pb.clone() }, &plan, yaw_from, horizon, SCALE);
    println!("follow: {} re-solves, {:.1} s, contacts {:?}, event {:?}", rep.resolves, rep.secs, &rep.contacts[..rep.contacts.len().min(6)], rep.event);
    let flown = rep.inputs;
    let row = |u: &sim::state::Input<f32>| InputRow { yaw: u.yaw as f64, pitch: u.pitch as f64, keys: "WR".to_string() };
    let mut inputs: Vec<InputRow> = base.rows[..t0].iter().map(|r| r.input.clone()).collect();
    inputs.extend(flown.iter().map(row));
    let l = *flown.last().ok_or("empty")?;
    inputs.extend((0..7).map(|_| row(&l)));
    let (tr, stopped) = fly(&base.initial, &inputs, &[], w, base.header.clone());
    let r = etas::verify::verify(course, w, etas::verify::Plan::Trajectory(&tr));
    let zz = |t: usize| state_at_full(&tr, t).position[2];
    let g = (t0.max(1)..tr.rows.len()).find(|&t| zz(t - 1) <= at && zz(t) > at);
    println!(
        "{out}: {} timed {:?} past the plane at row {g:?}, {:.3} ticks spare, stopped {stopped:?}, failure {:?}",
        if r.ok() { "PASS" } else { "FAIL" },
        r.timer_tick().zip(r.finish_tick()).map(|(s, e)| e - s),
        g.map_or(f64::NAN, |g| (zz(g) - at) / (zz(g) - zz(g - 1))),
        r.failures.first(),
    );
    if stopped.is_none() {
        tr.write(&dir.join(format!("runs/relax/{out}.traj.jsonl")))?;
    }
    Ok(())
}

fn smoothstep(u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// A, fading to B over ticks T0..T1, B aligned by z where the fade ends.
fn blend(dir: &Path, a: &str, b: &str, t0: usize, t1: usize, out: &str) -> Result<(), String> {
    let (a, b) = (relax(dir, a)?, relax(dir, b)?);
    let z = a.rows[t1 - 1].state.position[2];
    let shift = b.rows.iter().position(|r| r.state.position[2] < z).ok_or("no z")? as isize - (t1 as isize - 1);
    let mut rows = Vec::new();
    for (i, r) in a.rows.iter().enumerate().take(t1) {
        let mut r = r.clone();
        let w = smoothstep((i + 1).saturating_sub(t0) as f64 / (t1 - t0) as f64);
        if w > 0.0 {
            let o = &b.rows[(i as isize + shift) as usize];
            for k in 0..3 {
                r.state.position[k] += (o.state.position[k] - r.state.position[k]) * w;
                r.state.delta_movement[k] += (o.state.delta_movement[k] - r.state.delta_movement[k]) * w;
            }
            r.input.yaw += ((o.input.yaw - r.input.yaw + 180.0).rem_euclid(360.0) - 180.0) * w;
            r.input.pitch += (o.input.pitch - r.input.pitch) * w;
        }
        rows.push(r);
    }
    rows.extend(b.rows[(t1 as isize + shift) as usize..].iter().cloned());
    for (i, r) in rows.iter_mut().enumerate() {
        r.t = i + 1;
    }
    let mut h = a.header.clone();
    h.source = out.to_string();
    Trajectory { header: h, initial: a.initial.clone(), arrivals: Vec::new(), rows }.write(&dir.join(format!("runs/hybrid/{out}.traj.jsonl")))
}

fn bump(dir: &Path, base: &str, t0: usize, t1: usize, d: [f64; 3], out: &str) -> Result<(), String> {
    let mut t = relax(dir, base)?;
    let span = (t1 - t0) as f64;
    for r in &mut t.rows {
        if r.t > t0 && r.t < t1 {
            let u = (r.t - t0) as f64 / span;
            let s = (std::f64::consts::PI * u).sin();
            let (b, db) = (s * s, 2.0 * s * (std::f64::consts::PI * u).cos() * std::f64::consts::PI / span);
            for k in 0..3 {
                r.state.position[k] += d[k] * b;
                r.state.delta_movement[k] += d[k] * db;
            }
        }
    }
    t.header.source = out.to_string();
    t.write(&dir.join(format!("runs/hybrid/{out}.traj.jsonl")))
}

fn blocks(w: &World, a: &[f64]) {
    let (lo, hi): ([i32; 3], [i32; 3]) = (
        std::array::from_fn(|i| a[i].floor() as i32),
        std::array::from_fn(|i| a[i + 3].floor() as i32),
    );
    for y in lo[1]..=hi[1] {
        for z in lo[2]..=hi[2] {
            for x in lo[0]..=hi[0] {
                let pos = BlockPos { x, y, z };
                if let Cell::Known(id) = w.cell(pos) {
                    let name = &w.info(id).state;
                    if name.contains("minecraft:air") {
                        continue;
                    }
                    let b: Vec<String> = w.boxes_at(id, pos).iter().map(|q| format!("[{:.3} {:.3} {:.3} .. {:.3} {:.3} {:.3}]", q.min[0], q.min[1], q.min[2], q.max[0], q.max[1], q.max[2])).collect();
                    println!("{x} {y} {z} {name} {}", b.join(" "));
                }
            }
        }
    }
}

fn clear(dir: &Path, w: &World, name: &str, below: f64) -> Result<(), String> {
    let path = if name.contains('/') { name.to_string() } else { format!("runs/relax/{name}.traj.jsonl") };
    let t = Trajectory::read(&dir.join(path))?;
    let pts: Vec<[f64; 3]> = t.rows.iter().map(|r| r.state.position).collect();
    let walls = Walls::around(w, &pts, 6.0, 1.0);
    let first = t.rows.iter().position(|r| r.state.fall_flying).ok_or("no glide")?;
    let mut run: Option<(usize, usize, f64, [f64; 3], usize)> = None;
    let flush = |r: &(usize, usize, f64, [f64; 3], usize)| {
        let q = walls.boxes[r.4];
        println!(
            "t{}-{}: clearance {:.4}, at {:.2} {:.2} {:.2}, box {:.3} {:.3} {:.3} .. {:.3} {:.3} {:.3}",
            t.rows[r.0].t, t.rows[r.1].t, r.2, r.3[0], r.3[1], r.3[2], q.min[0], q.min[1], q.min[2], q.max[0], q.max[1], q.max[2]
        );
    };
    for i in first + 1..t.rows.len() {
        let (p, q) = (t.rows[i - 1].state.position, t.rows[i].state.position);
        // y, then the larger horizontal axis, then the other: as the physics moves
        let a = [p[0], q[1], p[2]];
        let b = if (q[0] - p[0]).abs() >= (q[2] - p[2]).abs() { [q[0], q[1], p[2]] } else { [p[0], q[1], q[2]] };
        let best = [(p, a), (a, b), (b, q)].iter().map(|l| walls.clearance(l.0, l.1)).min_by(|x, y| x.0.total_cmp(&y.0)).unwrap();
        if best.0 < below && let Some(n) = best.1 {
            match &mut run {
                Some(r) if r.1 + 1 == i => {
                    r.1 = i;
                    if best.0 < r.2 {
                        (r.2, r.3, r.4) = (best.0, q, n);
                    }
                }
                _ => {
                    if let Some(r) = &run {
                        flush(r);
                    }
                    run = Some((i, i, best.0, q, n));
                }
            }
        }
    }
    if let Some(r) = &run {
        flush(r);
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (course, w) = Course::load(dir)?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("blocks") => {
            let a: Vec<f64> = args[1..7].iter().map(|s| s.parse().unwrap()).collect();
            blocks(&w, &a);
            Ok(())
        }
        Some("clear") => clear(dir, &w, &args[1], args.get(2).map_or(0.05, |s| s.parse().unwrap())),
        Some("setup") => setup(dir, &w, &args[1]),
        Some("setupgrid") => setupgrid(dir, &w, &args[1]),
        Some("door") => door(dir, &w, &args[1]),
        Some("graft") => graft(dir, &course, &w, &args[1], &args[2], &args[3]),
        Some("calm") => {
            let weights: Vec<f64> = args[5..].iter().map(|s| s.parse().unwrap()).collect();
            calm(dir, &course, &w, &args[1], args[2].parse().unwrap(), args[3].parse().unwrap(), &args[4], &weights)
        }
        Some("finish") => finish(dir, &course, &w, &args[1], args[2].parse().unwrap(), &args[3], args[4].parse().unwrap()),
        Some("blend") => blend(dir, &args[1], &args[2], args[3].parse().unwrap(), args[4].parse().unwrap(), &args[5]),
        Some("bump") => {
            let v: Vec<f64> = args[4..7].iter().map(|s| s.parse().unwrap()).collect();
            bump(dir, &args[1], args[2].parse().unwrap(), args[3].parse().unwrap(), [v[0], v[1], v[2]], &args[7])
        }
        _ => Err("blocks | clear | setup | setupgrid | door | graft | blend | bump | calm | finish".into()),
    }
}
