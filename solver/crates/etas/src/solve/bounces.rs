//! Bounces: re-solving a recorded run whose ebounces lost the glide into one with
//! perfect bounces, and the lost-glide flights a plan branches to where a bounce's
//! glide-off comes at a tick no jump press rescues.
//!
//! Ticks are the trajectory's (`stretch`'s convention): the state "at t" is row `t`'s,
//! the initial state at 0, and a stretch `a..b` replaces the inputs of ticks
//! `a + 1 ..= b`. Unlike [`super::stretch::stretch`], a stretch here may pass through
//! ticks that aren't normal flight (a recording's lost glide and its redeploy); only
//! its ends must be. Everything here flies with port and applies the trajectory's
//! server arrivals, shifted when a stretch changes length.

use std::ops::RangeInclusive;

use port::{Unsupported, tick};
use sim::state::Input;
use world::World;

use super::ddp::{Goal, Opts, Problem, Solved, X, control, lead_of};
use super::refly::repair;
use super::stretch::{Stretch, resample};
use super::walls::Walls;
use crate::server::{self, Arrival, ServerEvent};
use crate::state::{FullState, keys_of, keys_to_string};
use crate::trajectory::{Header, InputRow, Row, Trajectory};

/// Errors over this scale count toward the reach tolerance: position in blocks,
/// velocity in blocks per 20 ticks (burrowed_grove's `resolve`).
pub const SCALE: X = [1.0, 1.0, 1.0, 0.05, 0.05, 0.05];
/// [`SCALE`] with the velocity left free: a position to reach.
pub const FREE_VELOCITY: X = [1.0, 1.0, 1.0, 1e9, 1e9, 1e9];
/// Infeasible horizons in a row [`reach_min_ticks`] tries past before it stops
/// shortening.
pub const PATIENCE: usize = 2;

pub fn state_at(traj: &Trajectory, t: usize) -> &FullState {
    if t == 0 {
        &traj.initial
    } else {
        &traj.rows[t - 1].state
    }
}

pub fn flight(s: &FullState) -> X {
    let (p, v) = (s.position, s.delta_movement);
    [p[0], p[1], p[2], v[0], v[1], v[2]]
}

/// Contact-free normal flight: gliding, off the ground, in the gliding pose, no
/// collision flags.
pub fn normal_flight(s: &FullState) -> bool {
    s.fall_flying
        && !s.on_ground
        && s.pose == "fall_flying"
        && !s.horizontal_collision
        && !s.vertical_collision
}

/// `inputs` flown with port from `initial`, `arrivals` (sorted) applied before their
/// ticks: the trajectory as far as port got, and the error that stopped it.
pub fn fly(
    initial: &FullState,
    inputs: &[InputRow],
    arrivals: &[Arrival],
    world: &World,
    header: Header,
) -> (Trajectory, Option<(usize, Unsupported)>) {
    let mut p = initial.to_player().expect("initial state");
    let mut rows = Vec::with_capacity(inputs.len());
    let mut err = None;
    for (i, input) in inputs.iter().enumerate() {
        let t = i + 1;
        server::apply_at(&mut p, arrivals, t, world);
        let ti = input.tick_input().expect("keys parse");
        if let Err(u) = tick(&mut p, &ti, world) {
            err = Some((t, u));
            break;
        }
        rows.push(Row {
            t,
            input: input.clone(),
            state: FullState::from(&p),
        });
    }
    (
        Trajectory {
            header,
            initial: initial.clone(),
            arrivals: arrivals.to_vec(),
            rows,
        },
        err,
    )
}

/// The stretch `a..b` of `rec` as a reach problem: from the state at `a` to the state
/// at `b`, seeded with the recorded rotations. Only the ends must be normal flight.
pub fn stretch_across(rec: &Trajectory, a: usize, b: usize) -> Result<Stretch, String> {
    if !(a < b && b <= rec.rows.len()) {
        return Err(format!("stretch {a}..{b} of {} ticks", rec.rows.len()));
    }
    for t in [a, b] {
        if !normal_flight(state_at(rec, t)) {
            return Err(format!("tick {t} is not contact-free normal flight"));
        }
    }
    Ok(stretch_between(
        rec,
        a,
        b,
        state_at(rec, a),
        state_at(rec, b),
    ))
}

/// A reach problem from `from` to `to`, seeded with `guide`'s rotations over `a..b`.
fn stretch_between(
    guide: &Trajectory,
    a: usize,
    b: usize,
    from: &FullState,
    to: &FullState,
) -> Stretch {
    let seed = (a..b)
        .map(|t| {
            let i = &guide.rows[t].input;
            control(lead_of(&flight(state_at(guide, t)), i.yaw), i.pitch)
        })
        .collect();
    Stretch {
        a,
        b,
        x0: flight(from),
        target: flight(to),
        yaw_from: from.y_rot,
        seed,
        path: (a..=b).map(|t| state_at(guide, t).position).collect(),
    }
}

/// What a reach solve came to.
#[derive(Debug, Clone)]
pub struct Reached {
    /// The repaired rotations, one per tick.
    pub inputs: Vec<Input<f32>>,
    /// The faithful end state's largest position and velocity errors from the target.
    pub pos_err: f64,
    pub vel_err: f64,
    /// Per horizon tried: its error and clearance, or why it was dropped.
    pub tried: Vec<String>,
}

fn walls_for(world: &World, pts: &[[f64; 3]]) -> Walls {
    Walls::around(world, pts, 4.0, 1.0)
}

/// Solve, growing the walls' region around the plan until it covers it.
fn solve_covered(
    world: &World,
    walls: &mut Walls,
    path: &[[f64; 3]],
    make: impl Fn(&Walls) -> Problem<'_>,
    seed: &[[f64; 2]],
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

/// The fewest ticks in `horizons` that reach `st.target` from `st.x0`, each coordinate
/// to within `scale` times the solve's tolerance (a huge velocity scale leaves the
/// velocity free): DDP through ghost walls on the generic kernel, from the stretch's
/// own length down (past up to [`PATIENCE`] infeasible horizons; up if none is
/// feasible), then repaired under the faithful kernel (`refly::repair`), a tick longer
/// while the repair touches anything. Fails, with what each horizon came to, if none
/// reaches it cleanly.
pub fn reach_min_ticks(
    world: &World,
    st: &Stretch,
    horizons: RangeInclusive<usize>,
    scale: &X,
    opts: &Opts,
) -> Result<Reached, String> {
    let kernel = sim::generic::Generic::default();
    let goal = Goal::Reach {
        target: st.target,
        scale: *scale,
    };
    let mut walls = walls_for(world, &st.path);
    let n0 = (st.b - st.a).clamp(*horizons.start(), *horizons.end());
    let mut tried = Vec::new();
    let solve = |walls: &mut Walls, n: usize, seed: &[[f64; 2]]| {
        solve_covered(
            world,
            walls,
            &st.path,
            |w| Problem {
                kernel,
                walls: w,
                x0: st.x0,
                goal: goal.clone(),
                opts: opts.clone(),
            },
            &resample(seed, n),
        )
    };
    // the least feasible horizon, searching from n0
    let mut best: Option<Solved> = None;
    let mut seed = st.seed.clone();
    let mut misses = 0;
    for n in (*horizons.start()..=n0).rev() {
        let s = solve(&mut walls, n, &seed);
        tried.push(format!(
            "{n}: {}",
            if s.feasible { "feasible" } else { "infeasible" }
        ));
        if !s.feasible {
            // the solve isn't monotone in the horizon: a shorter one can still work
            misses += 1;
            if misses > PATIENCE {
                break;
            }
            continue;
        }
        misses = 0;
        seed = s.plan.u.clone();
        best = Some(s);
    }
    if best.is_none() {
        for n in n0 + 1..=*horizons.end() {
            let s = solve(&mut walls, n, &seed);
            tried.push(format!(
                "{n}: {}",
                if s.feasible { "feasible" } else { "infeasible" }
            ));
            if s.feasible {
                best = Some(s);
                break;
            }
            seed = s.plan.u.clone();
        }
    }
    let Some(mut used) = best else {
        return Err(tried.join(", "));
    };
    loop {
        let pb = Problem {
            kernel,
            walls: &walls,
            x0: st.x0,
            goal: goal.clone(),
            opts: opts.clone(),
        };
        let rep = repair(world, &pb, &used, st.yaw_from, 100.0, 3);
        let n = used.plan.u.len();
        if rep.clean() && rep.inputs.len() == n {
            let end = *rep.flown.last().unwrap();
            let e: Vec<f64> = (0..6).map(|i| (end[i] - st.target[i]).abs()).collect();
            tried.push(format!("{n}: repaired"));
            return Ok(Reached {
                inputs: rep.inputs,
                pos_err: e[..3].iter().copied().fold(0.0, f64::max),
                vel_err: e[3..].iter().copied().fold(0.0, f64::max),
                tried,
            });
        }
        tried.push(format!(
            "{n}: repair touched ({:?})",
            rep.event.map(|e| e.0)
        ));
        if n >= *horizons.end() {
            return Err(tried.join(", "));
        }
        used = pb.solve(&resample(&used.plan.u, n + 1));
    }
}

/// `inputs` with ticks `a + 1 ..= b` replaced by `rotations`, whose keys are those of
/// the last `rotations.len()` ticks of the stretch (as `stretch::splice` does), with
/// jump released. why released? a recording's jump presses in the air are its
/// redeploys, which a perfect bounce doesn't make, and a held jump would take the edge
/// a reaction needs.
pub fn replace(inputs: &[InputRow], a: usize, b: usize, rotations: &[Input<f32>]) -> Vec<InputRow> {
    let n = rotations.len();
    let mut out: Vec<InputRow> = inputs[..a].to_vec();
    for (j, u) in rotations.iter().enumerate() {
        let src = (b + j + 1).saturating_sub(n).max(a + 1).min(b);
        let mut k = keys_of(&inputs[src - 1].keys).expect("keys parse");
        k.jump = false;
        out.push(InputRow {
            yaw: u.yaw as f64,
            pitch: u.pitch as f64,
            keys: keys_to_string(k),
        });
    }
    out.extend(inputs[b..].iter().cloned());
    out
}

/// `arrivals` for a trajectory whose ticks `a + 1 ..= b` became `n` ticks: those in
/// the stretch are dropped, later ones move by `n - (b - a)`.
pub fn shift_arrivals(arrivals: &[Arrival], a: usize, b: usize, n: usize) -> Vec<Arrival> {
    arrivals
        .iter()
        .filter(|x| x.tick <= a || x.tick > b)
        .map(|x| Arrival {
            tick: if x.tick > b {
                x.tick + n - (b - a)
            } else {
                x.tick
            },
            ..*x
        })
        .collect()
}

/// A stretch re-solved and spliced into `rec`, flown with port.
#[derive(Debug, Clone)]
pub struct Spliced {
    pub traj: Trajectory,
    pub stopped: Option<(usize, Unsupported)>,
    pub reached: Reached,
    /// Ticks the stretch got shorter by (negative: longer).
    pub saved: isize,
}

/// Re-solve `a..b` of `rec` for the fewest ticks in `horizons` and splice it in, the
/// stretch's server arrivals dropped (a glide-off there is one the new stretch
/// doesn't get: its bounce becomes perfect) and later ones shifted.
pub fn resolve_stretch(
    world: &World,
    rec: &Trajectory,
    a: usize,
    b: usize,
    horizons: RangeInclusive<usize>,
    opts: &Opts,
    header: Header,
) -> Result<Spliced, String> {
    let st = stretch_across(rec, a, b)?;
    let reached = reach_min_ticks(world, &st, horizons, &SCALE, opts)
        .map_err(|e| format!("stretch {a}..{b}: no horizon reaches the target cleanly ({e})"))?;
    let n = reached.inputs.len();
    let inputs: Vec<InputRow> = rec.rows.iter().map(|r| r.input.clone()).collect();
    let inputs = replace(&inputs, a, b, &reached.inputs);
    let arrivals = shift_arrivals(&rec.arrivals, a, b, n);
    let (traj, stopped) = fly(&rec.initial, &inputs, &arrivals, world, header);
    Ok(Spliced {
        traj,
        stopped,
        saved: (b - a) as isize - n as isize,
        reached,
    })
}

/// Controls that fly the generic kernel from `x0` along `guide`'s path from tick `a`
/// to `b`: each tick, of the controls near that of the guide's tick nearest the flown
/// position, the one whose move ends nearest the guide's path a few ticks on (held
/// for them), until the nearest tick is `b - 1`. why not the guide's controls by
/// tick? a lost glide runs behind and slower, so those drift off the path, into a wall
/// the ghost-wall solve then can't leave (it stays inside, its gradient along the
/// wall's thickness).
pub fn follow_seed(guide: &Trajectory, x0: &X, a: usize, b: usize) -> Vec<[f64; 2]> {
    const AHEAD: usize = 3;
    let kernel = sim::generic::Generic::default();
    let ctl = |u: usize| {
        let i = &guide.rows[u].input;
        control(lead_of(&flight(state_at(guide, u)), i.yaw), i.pitch)
    };
    let gap = |x: &X, j: usize| {
        (j..=(j + 6).min(b))
            .map(|u| {
                let p = state_at(guide, u).position;
                (0..3).map(|k| (p[k] - x[k]).powi(2)).sum::<f64>()
            })
            .fold(f64::INFINITY, f64::min)
    };
    let mut x = *x0;
    let mut j = a;
    let mut out = Vec::new();
    let steps = [-0.1, -0.04, -0.015, 0.0, 0.015, 0.04, 0.1];
    while j + 1 < b && out.len() < 4 * (b - a) {
        let d2 = |u: usize| {
            let p = state_at(guide, u).position;
            (0..3).map(|k| (p[k] - x[k]).powi(2)).sum::<f64>()
        };
        j = (j..=(j + 3).min(b - 1))
            .min_by(|&u, &v| d2(u).total_cmp(&d2(v)))
            .unwrap_or(j);
        let base = ctl(j);
        let mut best = (f64::INFINITY, base);
        for dl in steps {
            for dp in steps {
                let u = [base[0] + dl, (base[1] + 2.0 * dp).clamp(-1.5, 1.5)];
                let mut y = x;
                for _ in 0..AHEAD {
                    y = super::ddp::step(&kernel, &y, u).0;
                }
                let g = gap(&y, j);
                if g < best.0 {
                    best = (g, u);
                }
            }
        }
        out.push(best.1);
        x = super::ddp::step(&kernel, &x, best.1).0;
        j += 1;
    }
    out
}

/// Controls that fly the generic kernel from `x0` along `guide`'s path from tick `a`
/// to `b`, clear of the walls: a chase of short reaches, each to the guide's position
/// `CHASE` ticks past the guide's tick nearest the flown position (velocity free, the
/// fewest ticks that solve), keeping its first half. None if a reach fails. why? see
/// [`follow_seed`]: a long solve seeded off the path can't get back out of a wall,
/// and a short one from near the path can.
pub fn chase_seed(
    world: &World,
    guide: &Trajectory,
    x0: &X,
    a: usize,
    b: usize,
    opts: &Opts,
) -> Option<Vec<[f64; 2]>> {
    const CHASE: usize = 10;
    let kernel = sim::generic::Generic::default();
    let path: Vec<[f64; 3]> = (a..=b).map(|u| state_at(guide, u).position).collect();
    let walls = walls_for(world, &path);
    let mut x = *x0;
    let mut j = a;
    let mut out: Vec<[f64; 2]> = Vec::new();
    while j + 1 < b && out.len() < 4 * (b - a) {
        let d2 = |u: usize| {
            let p = state_at(guide, u).position;
            (0..3).map(|k| (p[k] - x[k]).powi(2)).sum::<f64>()
        };
        j = (j..=(j + 3).min(b - 1))
            .min_by(|&u, &v| d2(u).total_cmp(&d2(v)))
            .unwrap_or(j);
        let to = (j + CHASE).min(b);
        let mut target = flight(state_at(guide, to));
        target[3..].copy_from_slice(&x[3..]);
        let seed = follow_seed(guide, &x, j, to);
        let mut got = None;
        for n in (to - j).saturating_sub(2).max(2)..=(to - j) + 4 {
            let pb = Problem {
                kernel,
                walls: &walls,
                x0: x,
                goal: Goal::Reach {
                    target,
                    scale: FREE_VELOCITY,
                },
                opts: opts.clone(),
            };
            let s = pb.solve(&resample(&seed, n));
            if s.feasible {
                got = Some(s.plan);
                break;
            }
        }
        let plan = got?;
        let keep = if to == b {
            plan.u.len()
        } else {
            plan.u.len().div_ceil(2)
        };
        out.extend_from_slice(&plan.u[..keep]);
        x = plan.x[keep];
        j += keep;
        if to == b {
            break;
        }
    }
    Some(out)
}

/// A lost glide: what a plan's child segment flies when the glide-off of a bounce
/// comes before tick `t` and isn't rescued.
#[derive(Debug, Clone)]
pub struct Lost {
    /// The whole path: `main` up to `t - 1`, then the lost tick(s), the redeploy, a
    /// re-solved stretch to `main`'s state at `rejoin`, and `main`'s inputs after it.
    /// Its arrivals: the glide-off at `t`.
    pub traj: Trajectory,
    pub stopped: Option<(usize, Unsupported)>,
    /// The tick whose jump press restarts the glide: `t`, or `t + 1` when jump was
    /// held at `t - 1` (no edge).
    pub press: usize,
    /// The tick the re-solved stretch starts from: the press, or with `through_bounce`
    /// the first tick in normal flight after this path's own next bounce.
    pub from: usize,
    /// `main`'s tick whose state the re-solved stretch reaches, and the tick it does
    /// on this path.
    pub rejoin: usize,
    pub rejoined_at: usize,
    /// Whether it reaches only `main`'s position there (`lost_glide`'s `free_velocity`).
    pub free_velocity: bool,
    pub reached: Reached,
}

/// The lost-glide flight from `main`'s state before tick `t`, with a glide-off handled
/// before `t`'s physics (the server's pose, as `server::apply`): no glide until the
/// press (main's rotations and keys there, jump released before it), a press that
/// restarts it, then the fewest ticks that reach `main`'s flight state at `rejoin`
/// (horizons `extra` around each seed's length; seeds tried in turn: `main`'s controls
/// from its tick nearest the path's, [`follow_seed`], [`chase_seed`]), and `main`'s
/// inputs after `rejoin`. With
/// `through_bounce`, the path first flies `main`'s rotations open loop through its own
/// next landing, jumping on the tick after it (jump held on ticks that start on the
/// ground while gliding, as `trajectory::seed_perfect_bounces` does), and the stretch
/// starts once it is back in normal flight; `rejoin` is then a tick after `main`'s
/// next bounce. why? next to the ground a lost glide may have too little height left
/// to win back the speed before the main line's next landing, and the flight-only
/// solve can't plan a bounce. With `free_velocity` the stretch reaches `main`'s
/// position at `rejoin` only, at whatever velocity it can: for a lost glide with less
/// energy than the main line has anywhere after it (gliding only loses energy, so
/// after the last bounce no state of `main` is reachable exactly). `main` must carry
/// no arrivals after `t` (a plan's main line has none: its bounces are perfect).
#[allow(clippy::too_many_arguments)]
pub fn lost_glide(
    world: &World,
    main: &Trajectory,
    t: usize,
    rejoin: usize,
    through_bounce: bool,
    free_velocity: bool,
    extra: RangeInclusive<isize>,
    opts: &Opts,
    header: Header,
) -> Result<Lost, String> {
    if main.arrivals.iter().any(|a| a.tick >= t) {
        return Err(format!("main line has arrivals at or after tick {t}"));
    }
    let before = state_at(main, t - 1);
    let held = keys_of(&before.input_keys)
        .map_err(|e| format!("tick {}: {e}", t - 1))?
        .jump;
    let press = if held { t + 1 } else { t };
    if rejoin <= press + 2 {
        return Err(format!("rejoin {rejoin} too close to the press at {press}"));
    }
    let glide_off = [Arrival {
        tick: t,
        event: ServerEvent::GlideOff,
    }];
    let mut inputs: Vec<InputRow> = main.rows[..t - 1].iter().map(|r| r.input.clone()).collect();
    for u in t..=press {
        let r = &main.rows[u - 1].input;
        let mut k = keys_of(&r.keys)?;
        k.jump = u == press;
        inputs.push(InputRow {
            yaw: r.yaw,
            pitch: r.pitch,
            keys: keys_to_string(k),
        });
    }
    let (mut head, stopped) = fly(&main.initial, &inputs, &glide_off, world, header.clone());
    if let Some((u, e)) = stopped {
        return Err(format!("lost glide at {t}: port stopped at {u}: {e:?}"));
    }
    let at_press = &head.rows[press - 1].state;
    if !normal_flight(at_press) {
        return Err(format!(
            "lost glide at {t}: not in normal flight after the press at {press} (gliding {}, ground {}, pose {})",
            at_press.fall_flying, at_press.on_ground, at_press.pose
        ));
    }
    let mut from = press;
    if through_bounce {
        let mut p = at_press.to_player()?;
        let mut landed = false;
        loop {
            let u = from + 1;
            if u + 3 > rejoin {
                return Err(format!(
                    "lost glide at {t}: no bounce of its own before tick {rejoin}"
                ));
            }
            let r = &main.rows[u - 1].input;
            let mut k = keys_of(&r.keys)?;
            k.jump = p.fall_flying && p.on_ground;
            let input = InputRow {
                yaw: r.yaw,
                pitch: r.pitch,
                keys: keys_to_string(k),
            };
            tick(&mut p, &input.tick_input()?, world)
                .map_err(|e| format!("lost glide at {t}: port stopped at {u}: {e:?}"))?;
            let s = FullState::from(&p);
            landed |= s.on_ground;
            inputs.push(input.clone());
            head.rows.push(Row {
                t: u,
                input,
                state: s.clone(),
            });
            from = u;
            if landed && normal_flight(&s) {
                break;
            }
        }
    }
    let at_from = &head.rows[from - 1].state;
    // seed from where the main line is nearest: a lost glide runs behind it
    let d2 = |u: usize| {
        let (p, q) = (state_at(main, u).position, at_from.position);
        (0..3).map(|k| (p[k] - q[k]).powi(2)).sum::<f64>()
    };
    let near = (from.saturating_sub(6).max(1)..=(from + 6).min(rejoin - 3))
        .filter(|&u| normal_flight(state_at(main, u)))
        .min_by(|&u, &v| d2(u).total_cmp(&d2(v)))
        .unwrap_or(from);
    let mut st = stretch_between(main, near, rejoin, at_from, state_at(main, rejoin));
    st.a = from;
    let scale = if free_velocity { FREE_VELOCITY } else { SCALE };
    // two seeds: the main line's controls from the nearest tick, then the path follower
    let by_tick = st.seed.clone();
    let mut errors = Vec::new();
    let mut found = None;
    let mut seeds = vec![
        ("by tick", by_tick),
        ("follow", follow_seed(main, &st.x0, near, rejoin)),
    ];
    if let Some(c) = chase_seed(world, main, &st.x0, near, rejoin, opts) {
        seeds.push(("chase", c));
    }
    for (name, seed) in seeds {
        let n0 = seed.len() as isize;
        let lo = (n0 + extra.start()).max(3) as usize;
        let hi = (n0 + extra.end()).max(lo as isize) as usize;
        st.seed = seed;
        match reach_min_ticks(world, &st, lo..=hi, &scale, opts) {
            Ok(r) => {
                found = Some(r);
                break;
            }
            Err(e) => errors.push(format!("seed {name}, horizons {lo}..={hi}: {e}")),
        }
    }
    let reached = found.ok_or_else(|| {
        format!(
            "lost glide at {t}: from {from} (at {:?}, velocity {:?}) nothing reaches tick {rejoin} ({})",
            at_from.position,
            at_from.delta_movement,
            errors.join("; ")
        )
    })?;
    let keys: Vec<InputRow> = main.rows.iter().map(|r| r.input.clone()).collect();
    // keys for the new stretch as `replace` picks them, from main's ticks before rejoin
    let stretch = replace(&keys, from, rejoin, &reached.inputs);
    inputs.extend(stretch[from..from + reached.inputs.len()].iter().cloned());
    inputs.extend(main.rows[rejoin..].iter().map(|r| r.input.clone()));
    let (traj, stopped) = fly(&main.initial, &inputs, &glide_off, world, header);
    Ok(Lost {
        traj,
        stopped,
        press,
        from,
        rejoin,
        rejoined_at: from + reached.inputs.len(),
        free_velocity,
        reached,
    })
}
