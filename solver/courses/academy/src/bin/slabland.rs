//! Phase `slabland`: from the reset pose to standing on the slab, by ebounces, joined to a flight off the slab.
//!
//! The flights `dome slabflights` and `optimize` make start standing on the slab. Getting there takes a landing,
//! whose glide-off comes at a tick that depends on ping, so a plan has to go on from each arrival.
//!
//! - `map`: the room's blocks, by layer.
//! - `search BOUNCES...`: the fewest ticks from the reset to the slab, by rotations under port, for each number of
//!   bounces on the floor, seeded by what the recordings do. Writes `runs/slabland/search.json`.
//! - `join FLIGHT`: that way, then per arrival of the slab's glide-off a walk to rest on the flight's spot, then
//!   `runs/relax/FLIGHT`, as the plan `runs/slabland/FLIGHT.tas`, with every path verified.
//!
//! The recordings: 348 attempts land on the slab from the reset with a glide, 138 with exact poses, nearly all one
//! bounce at z 215-216 and landing 47-50 ticks after the reset. The ones that land slowly enough look back north
//! first, which brakes a glide. No recording is spliced in: they fly one route, and this beats them all from its shape.
//!
//! What it has to respect: a landing over 0.3 b/t into the back wall comes back as damage and a velocity, a glide
//! stays on the ground only looking down, and over the slab's back edge only x -25.894 to -25.106 has the chain
//! under it. The arrival is unknown until it comes, and keys do nothing until then, so each has its own walk.

use std::path::Path;
use std::process::ExitCode;

use port::tick;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use etas::course::Course;
use etas::plan::{Action, Event, PathTape, Plan, React, branched, plan_from, verify_paths};
use etas::server::Arrival;
use etas::solve::bounces::{flight as flight_of, fly, state_at};
use etas::solve::ddp::{X, server_hits};
use etas::solve::walls::Walls;
use etas::state::FullState;
use etas::server::{self, ServerEvent};
use etas::tape::Tape;
use etas::trajectory::{Header, InputRow, Trajectory, from_tape};
use etas::verify::{GLIDE_OFF_MAX, GLIDE_OFF_MIN, GLIDE_OFF_NATURAL, Plan as VPlan, verify};
use world::{Aabb, World};

/// A recorded tape that starts at the reset pose, for its initial state.
const RESET: &str = "runs/seeds/34.020_20261103_22_4_run10.tas";
/// How far apart a rotation's knots are, and how many.
const KNOT: usize = 6;
const KNOTS: usize = 12;
const CAP: usize = KNOT * (KNOTS - 1);
/// Landings closer than this have glide-offs that can come during the next one. why 10? `GLIDE_OFF_MAX` is 8.
const SPACING: usize = 10;
/// The slab's lip: feet on anything from here south, at the slab's height, count as landed.
const LIP_Z: f64 = 226.7;
const LAND_Y: f64 = 283.4;
/// The takeoff spot of the flights off the slab, and the walking speed a way is priced at to get there.
const SPOT: [f64; 3] = [-25.8, 283.594, 227.9];
const WALK: f64 = 0.2;
/// As far south as a standing player fits, under the roof; a glide on the chain stops there too.
const BACK_Z: f64 = 228.71;
/// The middle of the slab along x, over the chain behind it.
const SLAB_X: f64 = -25.5;
const ROUNDS: usize = 40;
const POP: usize = 1500;
const ELITE: usize = 80;
const RESTARTS: usize = 4;
const OBJECTIVE: &str = "finish_gate_segments";
/// Ticks standing still that a walk gets, to stop. why not fewer? what's left of a velocity zeroes under 0.003.
const STILL: usize = 16;
/// How close to the flight's spot a yaw per half of the walk gets, blocks. why no closer? `Mth.sin` is a table, so a
/// tick's yaw moves a walk in steps about 1e-5 of a block.
const WALK_TOL: f64 = 4e-5;
/// How close per-tick yaws then get, by that table's steps, and how many tries they get. why so close? an offset at the
/// spot grows along the flight; see `join`'s print.
const FINE_TOL: f64 = 2e-8;
const FINE_TRIES: usize = 6000;
const TABLE_STEP: f64 = 360.0 / 65536.0;
/// Ticks after the flight's last, as in `tas`.
const TAIL: usize = 7;
/// The most horizontal speed a wall may take, under the 0.3 b/t the server answers with damage and a velocity.
const HARD: f64 = 0.25;

/// From a landing, the glide-off's earliest and latest arrivals we plan for: no keys, looking back, on the ground
/// until it stops. Returns how far from the spot it rests, or how far it went wrong.
fn slide(p0: &port::LocalPlayer, w: &World) -> Result<f64, f64> {
    // looking down, or the glide lifts it back off the slab
    let idle = InputRow { yaw: 180.0, pitch: 60.0, keys: "-".to_string() }.tick_input().map_err(|_| 50.0)?;
    let mut worst = 0.0f64;
    for k in [GLIDE_OFF_NATURAL[0], GLIDE_OFF_MAX] {
        let mut p = p0.clone();
        let mut before = FullState::from(&p);
        for t in 1..=24 {
            if t == k {
                server::apply(&mut p, &ServerEvent::GlideOff, w);
            }
            tick(&mut p, &idle, w).map_err(|_| 50.0)?;
            let s = FullState::from(&p);
            let speed = |x: &FullState| x.delta_movement[0].hypot(x.delta_movement[2]);
            if s.horizontal_collision && speed(&before) - speed(&s) > HARD {
                return Err(10.0 + 20.0 * (speed(&before) - speed(&s) - HARD));
            }
            if !s.on_ground || s.position[1] < LAND_Y {
                // off the back or the side: over the chain, or slower
                return Err(10.0 + 6.0 * (s.position[0] - SLAB_X).abs() + 4.0 * speed(&before) - 0.1 * t as f64);
            }
            before = s;
        }
        if before.position[2] > BACK_Z {
            return Err(10.0 + (before.position[2] - BACK_Z) * 5.0);
        }
        worst = worst.max((before.position[0] - SPOT[0]).hypot(before.position[2] - SPOT[2]));
    }
    Ok(worst)
}

/// run, air, then knots of pitch, then knots of yaw
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Way {
    bounces: usize,
    run: usize,
    air: usize,
    pitch: Vec<f64>,
    yaw: Vec<f64>,
    /// From this z on it looks back north, which brakes a glide: what the recordings do before they land.
    #[serde(default = "no_brake")]
    brake_z: f64,
}

fn no_brake() -> f64 {
    240.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Found {
    way: Way,
    score: f64,
    land: usize,
    pos: [f64; 3],
    vel: [f64; 3],
    bounces_at: Vec<usize>,
    inputs: Vec<InputRow>,
}

fn knot(k: &[f64], t: usize) -> f64 {
    let u = t as f64 / KNOT as f64;
    let i = (u as usize).min(k.len() - 2);
    k[i] + (k[i + 1] - k[i]) * (u - i as f64)
}

/// The flight of a way: until it lands on the slab, or the cap.
fn fly_way(initial: &FullState, w: &World, way: &Way) -> Option<Found> {
    let mut p = initial.to_player().ok()?;
    let mut inputs = Vec::new();
    let mut bounces_at = Vec::new();
    let (mut last_land, mut was_ground, mut held) = (None::<usize>, false, false);
    let (mut gliding, mut deployed) = (false, false);
    let mut s = initial.clone();
    for t in 1..=CAP {
        let jump_tick = way.run + 1;
        let deploy_tick = jump_tick + way.air;
        let mut j = t == jump_tick || t == deploy_tick;
        if t == deploy_tick {
            deployed = true;
        }
        // a bounce: the tick after a gliding landing on the floor
        if was_ground && gliding && deployed && t > deploy_tick && !held {
            j = true;
        }
        let keys = if j { "WRJ" } else { "WR" };
        let yaw = if s.position[2] > way.brake_z { 180.0 } else { knot(&way.yaw, t) };
        let row = InputRow { yaw, pitch: knot(&way.pitch, t).clamp(-90.0, 90.0), keys: keys.to_string() };
        let ti = row.tick_input().ok()?;
        tick(&mut p, &ti, w).ok()?;
        inputs.push(row);
        held = j;
        s = FullState::from(&p);
        gliding = s.fall_flying;
        let on = s.on_ground;
        if on && gliding && !was_ground {
            if s.position[1] >= LAND_Y && s.position[2] >= LIP_Z {
                let extra = match slide(&p, w) {
                    Ok(d) => d / WALK,
                    Err(e) => e,
                };
                return Some(Found {
                    way: way.clone(),
                    score: t as f64 + extra + 8.0 * (bounces_at.len() as f64 - way.bounces as f64).abs(),
                    land: t,
                    pos: s.position,
                    vel: s.delta_movement,
                    bounces_at,
                    inputs,
                });
            }
            if last_land.is_some_and(|l: usize| t - l < SPACING) || bounces_at.len() >= way.bounces {
                break;
            }
            last_land = Some(t);
            bounces_at.push(t);
        }
        was_ground = on;
        if s.position[1] < 281.0 || s.horizontal_collision {
            break;
        }
    }
    let d = ((s.position[0] + 25.8).powi(2) + (s.position[1] - 283.5).powi(2) + (s.position[2] - 227.3).powi(2)).sqrt();
    Some(Found {
        way: way.clone(),
        score: 100.0 + d + 5.0 * (bounces_at.len() as f64 - way.bounces as f64).abs(),
        land: 0,
        pos: s.position,
        vel: s.delta_movement,
        bounces_at,
        inputs,
    })
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn normal(&mut self) -> f64 {
        let (a, b) = (self.next().max(1e-12), self.next());
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

fn to_way(bounces: usize, v: &[f64]) -> Way {
    Way {
        bounces,
        run: v[0].round().clamp(0.0, 12.0) as usize,
        air: v[1].round().clamp(1.0, 12.0) as usize,
        pitch: v[2..2 + KNOTS].to_vec(),
        yaw: v[2 + KNOTS..2 + 2 * KNOTS].to_vec(),
        brake_z: v[2 + 2 * KNOTS],
    }
}

/// The recordings' shape: a run of 6, 5 ticks in the air, down to the floor, up, over, and down onto the slab.
fn seed(bounces: usize) -> Vec<f64> {
    let mut v = vec![6.0, 5.0];
    let pitch: Vec<f64> = match bounces {
        0 => vec![30.0, 30.0, 5.0, -10.0, -5.0, 5.0, 15.0, 30.0, 35.0, 40.0, 40.0, 40.0],
        1 => vec![30.0, 35.0, 30.0, 12.0, -5.0, 0.0, 10.0, 25.0, 40.0, 40.0, 40.0, 40.0],
        _ => (0..KNOTS).map(|i| if i * KNOT < 20 { 45.0 } else if i * KNOT < 34 { 0.0 } else { 20.0 }).collect(),
    };
    v.extend(pitch);
    v.extend(std::iter::repeat_n(-3.5, KNOTS));
    v.push(226.0);
    v
}

fn search(dir: &Path, initial: &FullState, w: &World, counts: &[usize]) -> Result<(), String> {
    let mut all: Vec<Found> = Vec::new();
    for &b in counts {
        for r in 0..RESTARTS {
            let mut mean = seed(b);
            let mut sd: Vec<f64> = vec![2.0, 2.0];
            sd.extend(std::iter::repeat_n(if r == 0 { 12.0 } else { 25.0 }, KNOTS));
            sd.extend(std::iter::repeat_n(6.0, KNOTS));
            sd.push(1.5);
            let mut best: Option<Found> = None;
            for round in 0..ROUNDS {
                let scored: Vec<(f64, Vec<f64>, Found)> = (0..POP)
                    .into_par_iter()
                    .filter_map(|i| {
                        let mut g = Lcg((b * 1000 + r * 100 + round) as u64 * 1_000_003 + i as u64 * 7_777_777);
                        for _ in 0..4 {
                            g.next();
                        }
                        let v: Vec<f64> = mean.iter().zip(&sd).map(|(m, s)| m + s * g.normal()).collect();
                        let f = fly_way(initial, w, &to_way(b, &v))?;
                        Some((f.score, v, f))
                    })
                    .collect();
                let mut scored = scored;
                scored.sort_by(|a, b| a.0.total_cmp(&b.0));
                if best.as_ref().is_none_or(|x| scored[0].0 < x.score) {
                    best = Some(scored[0].2.clone());
                }
                let e = &scored[..ELITE.min(scored.len())];
                for k in 0..mean.len() {
                    let m = e.iter().map(|x| x.1[k]).sum::<f64>() / e.len() as f64;
                    let var = e.iter().map(|x| (x.1[k] - m).powi(2)).sum::<f64>() / e.len() as f64;
                    mean[k] = m;
                    sd[k] = 0.85 * sd[k] * 0.3 + 0.7 * var.sqrt() + 0.05;
                }
            }
            let f = best.ok_or("nothing flew")?;
            println!(
                "bounces {b} try {r}: land {} score {:.2} at {:.2} {:.2} {:.2} v {:.2} {:.2} {:.2}, run {} air {}, bounces at {:?}, brake z {:.2}",
                f.land, f.score, f.pos[0], f.pos[1], f.pos[2], f.vel[0], f.vel[1], f.vel[2], f.way.run, f.way.air, f.bounces_at, f.way.brake_z
            );
            all.push(f);
        }
    }
    all.sort_by(|a, b| a.score.total_cmp(&b.score));
    std::fs::create_dir_all(dir.join("runs/slabland")).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("runs/slabland/search.json"), serde_json::to_string(&all).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(())
}

/// From `p`, just as the glide-off lands: `n` ticks of W at yaws going from `a` to `b` halfway, then still.
fn walk_rows(a: f64, b: f64, n: usize) -> Vec<InputRow> {
    let mut v: Vec<InputRow> = (0..n)
        .map(|i| InputRow { yaw: if i * 2 < n { a } else { b }, pitch: 0.0, keys: "W".to_string() })
        .collect();
    v.extend((0..STILL).map(|_| InputRow { yaw: b, pitch: 0.0, keys: "-".to_string() }));
    v
}

fn walk_end(p0: &port::LocalPlayer, w: &World, rows: &[InputRow]) -> Option<([f64; 3], usize)> {
    let mut p = p0.clone();
    let mut last = None;
    for (i, r) in rows.iter().enumerate() {
        tick(&mut p, &r.tick_input().ok()?, w).ok()?;
        let s = FullState::from(&p);
        if s.position[1] < LAND_Y {
            return None;
        }
        let moved = s.delta_movement[0] != 0.0 || s.delta_movement[2] != 0.0;
        if moved || last.is_none() {
            last = Some(i + 1);
        }
    }
    Some((FullState::from(&p).position, last?))
}

/// The first `n` rows' yaws moved by the sine table's steps, pairs at a time, while it gets closer to the spot.
fn fine(p0: &port::LocalPlayer, w: &World, rows: &mut [InputRow], n: usize) {
    let off = |rows: &[InputRow]| walk_end(p0, w, rows).map_or(f64::MAX, |(q, _)| (q[0] - SPOT[0]).hypot(q[2] - SPOT[2]));
    let mut best = off(rows);
    let mut g = Lcg(0x5eed);
    for _ in 0..FINE_TRIES {
        if best < FINE_TOL {
            break;
        }
        let (i, j) = ((g.next() * n as f64) as usize, (g.next() * n as f64) as usize);
        let (di, dj) = ((g.next() * 13.0).floor() - 6.0, (g.next() * 13.0).floor() - 6.0);
        let (yi, yj) = (rows[i].yaw, rows[j].yaw);
        rows[i].yaw += di * TABLE_STEP;
        if i != j {
            rows[j].yaw += dj * TABLE_STEP;
        }
        let e = off(rows);
        if e < best {
            best = e;
        } else {
            rows[i].yaw = yi;
            rows[j].yaw = yj;
        }
    }
}

/// Yaws that walk from `p0` to rest on the spot. The fewest ticks that get there.
fn walk_to_spot(p0: &port::LocalPlayer, w: &World) -> Option<Vec<InputRow>> {
    let s0 = FullState::from(p0);
    let dir = (-(SPOT[0] - s0.position[0])).atan2(SPOT[2] - s0.position[2]).to_degrees();
    for n in 1..=24 {
        for spread in [8.0, 25.0, 50.0, 70.0] {
            let (mut a, mut b) = (dir - spread, dir + spread);
            let err = |a: f64, b: f64| -> Option<([f64; 2], usize)> {
                let (q, m) = walk_end(p0, w, &walk_rows(a, b, n))?;
                Some(([q[0] - SPOT[0], q[2] - SPOT[2]], m))
            };
            for _ in 0..40 {
                let Some((e, m)) = err(a, b) else { break };
                if e[0].hypot(e[1]) < WALK_TOL {
                    let mut rows = walk_rows(a, b, n);
                    // to the tick after it stops
                    rows.truncate(m + 2);
                    fine(p0, w, &mut rows, n);
                    return Some(rows);
                }
                let h = 0.05;
                let (Some((ea, _)), Some((eb, _))) = (err(a + h, b), err(a, b + h)) else { break };
                let (j00, j10, j01, j11) = ((ea[0] - e[0]) / h, (ea[1] - e[1]) / h, (eb[0] - e[0]) / h, (eb[1] - e[1]) / h);
                let det = j00 * j11 - j01 * j10;
                if det.abs() < 1e-9 {
                    break;
                }
                let (da, db) = ((j11 * e[0] - j01 * e[1]) / det, (j00 * e[1] - j10 * e[0]) / det);
                let cap = (da.abs().max(db.abs()) / 15.0).max(1.0);
                a -= da / cap;
                b -= db / cap;
            }
        }
    }
    None
}

/// `join FLIGHT`: the search's best way to the slab, each arrival's walk to the spot, then `runs/relax/FLIGHT`.
fn join(dir: &Path, course: &Course, world: &World, initial: &FullState, name: &str) -> Result<(), String> {
    let all: Vec<Found> = serde_json::from_str(&std::fs::read_to_string(dir.join("runs/slabland/search.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let way = all.first().ok_or("no way in search.json")?;
    let flight = Trajectory::read(&dir.join(format!("runs/relax/{name}.traj.jsonl")))?;
    let vf = verify(course, world, VPlan::Trajectory(&flight));
    let finish = vf.finish_tick().ok_or("flight has no finish")?;
    println!("{name}: timed {}", finish - vf.timer_tick().ok_or("no timer")?);
    // the flight's moves from the first that isn't standing still, to the finish and on
    let first = flight.rows.iter().position(|r| r.input.keys != "-").ok_or("flight never moves")?;
    let spot = state_at(&flight, first).clone();
    println!("flight leaves from {:?} at its tick {}", spot.position, first + 1);
    let mut head: Vec<InputRow> = flight.rows[first..finish - 1].iter().map(|r| r.input.clone()).collect();
    let last = head.last().ok_or("empty flight")?.clone();
    head.extend((0..TAIL).map(|_| last.clone()));
    let land = way.land;

    let mut p = initial.to_player().map_err(|e| format!("{e:?}"))?;
    for r in &way.inputs {
        tick(&mut p, &r.tick_input().map_err(|e| format!("{e:?}"))?, world).map_err(|e| format!("{e:?}"))?;
    }
    let idle = InputRow { yaw: 180.0, pitch: 60.0, keys: "-".to_string() };
    let header = || Header::new(Some(course), OBJECTIVE, &format!("slabland: ebounce to the slab, then {name}"));
    let mut trajs = Vec::new();
    for k in GLIDE_OFF_MIN..=GLIDE_OFF_MAX {
        let at = land + k;
        let mut inputs = way.inputs.clone();
        let mut q = p.clone();
        for _ in 1..k {
            tick(&mut q, &idle.tick_input().map_err(|e| format!("{e:?}"))?, world).map_err(|e| format!("{e:?}"))?;
            inputs.push(idle.clone());
        }
        server::apply(&mut q, &ServerEvent::GlideOff, world);
        let walk = walk_to_spot(&q, world).ok_or(format!("arrival {k}: no walk to the spot"))?;
        inputs.extend(walk.iter().cloned());
        let rest = inputs.len();
        inputs.extend(head.iter().cloned());
        let arrivals = [Arrival { tick: at, event: ServerEvent::GlideOff }];
        let (t, stopped) = fly(initial, &inputs, &arrivals, world, header());
        if let Some((t, u)) = stopped {
            return Err(format!("arrival {k}: port stopped at {t}: {u:?}"));
        }
        let at_rest = &t.rows[rest - 1].state;
        let off: f64 = (0..3).map(|i| (at_rest.position[i] - spot.position[i]).powi(2)).sum::<f64>().sqrt();
        let r = verify(course, world, VPlan::Trajectory(&t));
        let (shift, end) = (rest as isize - first as isize, finish - 1);
        let (here, there) = (t.rows[(end as isize + shift) as usize - 1].state.position, flight.rows[end - 1].state.position);
        let drift: f64 = (0..3).map(|i| (here[i] - there[i]).powi(2)).sum::<f64>().sqrt();
        println!("  off the flight at the pose before its finish: {drift:.1e} blocks, z {:.4}", here[2]);
        println!(
            "arrival {k} (tick {at}): {} ticks of walk and rest, rest tick {rest}, off the spot {off:.1e}, v {:?}, sprint {}; {}: timer {:?}, finish {:?}, {:?}",
            walk.len(),
            at_rest.delta_movement,
            at_rest.sprinting,
            if r.ok() { "PASS" } else { "FAIL" },
            r.timer_tick(),
            r.finish_tick(),
            r.failures.first()
        );
        trajs.push(t);
    }
    // the bounce's reaction, from a path's own report
    let (base, report) = plan_from(course, world, &trajs[1]);
    println!("bounces: {:?}", report.bounces.iter().map(|b| (b.landing, b.jump, b.window, &b.lost, b.not_rescuable.len())).collect::<Vec<_>>());
    let reacts: Vec<React> = report
        .bounces
        .iter()
        .filter(|b| b.jump.is_some() && b.lost.is_none())
        .filter_map(|b| b.window_to_plan())
        .map(|(t0, t1)| React { t0, t1, event: Event::GlideOff, action: Action::PressJump })
        .collect();
    println!("reacts: {reacts:?}");
    let paths: Vec<PathTape> = trajs.iter().map(|t| PathTape { rows: t.to_tape().rows, arrivals: t.arrivals.clone() }).collect();
    let plan = branched(&base.tape, reacts, &paths)?;
    let out = dir.join("runs/slabland");
    let file = out.join(format!("{name}.tas"));
    std::fs::write(&file, plan.write()).map_err(|e| e.to_string())?;
    let read = Plan::read(&file)?;
    let mut ok = true;
    for (path, r) in verify_paths(course, world, &read) {
        let timed = r.timer_tick().zip(r.finish_tick()).map(|(s, f)| f - s);
        println!(
            "plan path {:?} arrivals {:?}: {} timed {:?} ({}), timer tick {:?}, failures {:?}",
            path.leaf,
            path.arrivals.iter().map(|a| a.tick).collect::<Vec<_>>(),
            if r.ok() { "PASS" } else { "FAIL" },
            timed,
            timed.map_or("-".into(), |t| format!("{:.2} s", t as f64 * 0.05)),
            r.timer_tick(),
            r.failures.first()
        );
        ok &= r.ok();
    }
    // the server's box over the flight: not the planned landing, nor from the move before the finish on
    for t in &trajs {
        let flown: Vec<X> = t.rows.iter().map(|r| flight_of(&r.state)).collect();
        let pts: Vec<[f64; 3]> = t.rows.iter().map(|r| r.state.position).collect();
        let walls = Walls::around(world, &pts, 6.0, 1.0);
        let end = verify(course, world, VPlan::Trajectory(t)).finish_tick().unwrap_or(flown.len());
        let hits: Vec<_> = server_hits(&walls, &flown)
            .into_iter()
            .map(|(i, c)| (t.rows[i].t, c))
            .filter(|&(i, _)| i + 1 < end && t.rows[i - 1].state.fall_flying)
            .collect();
        println!("arrival tick {:?}: server hits {hits:?}", t.arrivals[0].tick);
        ok &= hits.iter().all(|&(i, _)| i <= land);
    }
    println!("{} {}", file.display(), if ok { "OK" } else { "NOT OK" });
    trajs[1].write(&out.join(format!("{name}.traj.jsonl")))?;
    Ok(())
}

/// What stops a player's feet at each column.
fn map(w: &World) {
    for y0 in 282..=288 {
        println!("layer y {y0}: x from -38 across, z down; digit tenths of box top in this layer, # full, c narrow, . air");
        for z in (205..=232).rev() {
            let mut s = format!("{z:4} ");
            for x in -38..=-15 {
                let a = Aabb { min: [x as f64, y0 as f64, z as f64], max: [x as f64 + 1.0, y0 as f64 + 1.0, z as f64 + 1.0] };
                let b = w.collision_boxes(a).unwrap_or_default();
                let c = if b.is_empty() {
                    '.'.to_string()
                } else {
                    let top = b.iter().map(|q| q.max[1]).fold(f64::MIN, f64::max) - y0 as f64;
                    let wx = b.iter().map(|q| q.max[0] - q.min[0]).fold(0.0, f64::max);
                    if top >= 1.0 && wx >= 1.0 { "#".to_string() } else if wx < 1.0 { "c".to_string() } else { format!("{}", (top * 10.0).round() as i32) }
                };
                s += &c;
            }
            println!("{s}");
        }
    }
}

fn run() -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (course, world) = Course::load(dir)?;
    let tape = Tape::read(&dir.join(RESET))?;
    let (rest, _) = from_tape(&tape, &world, Some(&course), RESET);
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("map") => map(&world),
        Some("search") => {
            let counts: Vec<usize> = args[1..].iter().filter_map(|a| a.parse().ok()).collect();
            search(dir, &rest.initial, &world, &counts)?;
        }
        Some("join") => join(dir, &course, &world, &rest.initial, args.get(1).ok_or("want a flight")?)?,
        _ => return Err("want a phase: map, search, join".into()),
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("slabland: {e}");
            ExitCode::from(2)
        }
    }
}
