//! `verify`: replay a trajectory or a `.tas` plan with port against a course's world
//! and rules, and say whether it holds. The judge is the least clever code we have
//! (ARCHITECTURE.md, Verify): port, the world file, and the course's predicates over
//! each tick's end pose, nothing from the solver that made the plan.
//!
//! **Perfect bounces.** A gliding landing inside the course's ground-allowed region
//! is a planned perfect bounce: port keeps gliding (only the server ends a glide), so
//! the replay already flies the bounce as if the server never undeployed. What makes
//! that true on EMC is the plan's `@react glide_off press_jump`: the glide-off arrives
//! a few ticks after the landing, and a jump press on that tick restarts the glide
//! before the tick's physics. For each tick of the window verify replays that branch
//! (glide flag off and the server's pose before the tick, jump held for it) and
//! requires it to fly the plan's poses. A gliding landing anywhere else fails: the
//! glide ends there.

use std::fmt;

use port::{LocalPlayer, TickInput, Unsupported, tick};
use serde::Serialize;
use world::{Aabb, World};

use crate::course::{Course, Observed, Rule, Variant, rest_state};
use crate::plan::{Action, Event, React};
use crate::state::FullState;
use crate::tape::Tape;
use crate::trajectory::Trajectory;

/// What verify replays. A trajectory carries no reactions, so its bounces are
/// reported with the window they need but not failed for lacking one; a tape is a
/// plan without reactions.
pub enum Plan<'a> {
    Trajectory(&'a Trajectory),
    Tape(&'a Tape),
    Reactive(&'a crate::plan::Plan),
}

/// The glide-off reaches the client at least this many ticks after the landing tick
/// (the tick whose move first says onGround). why 2? 26.2's server ticks levels, which
/// send entity data, before player connections, whose `doTick` ends the glide, so its
/// "off" leaves a tick after the tick that saw the ground (undeploy README: 2 is zero
/// ping in the best tick phase).
pub const GLIDE_OFF_MIN: usize = 2;
/// The latest glide-off a plan's window covers. why 8? on EMC it came 3 or 4 ticks
/// after the landing at ~65 ms round trip (2026-10-29 17:05 log: 3 on 112 bounces, 4
/// on 86), 4 in the 2026-10-19 ebounce replay, and 6 or 7 behind Ping Equalizer's
/// 68 ms hold (17:05 and 17:17 logs); one more for a packet read on either side of a
/// tick edge. Later than that the server is lagging, which makes bounces perfect on
/// its side anyway (README, "Things that will bite").
pub const GLIDE_OFF_MAX: usize = 8;
/// Delays a bounce's window must cover: those seen on EMC without a packet hold.
pub const GLIDE_OFF_NATURAL: [usize; 2] = [3, 4];

/// A rule's first firing: the tick whose end pose the server judged, and the secant
/// time: where the segment from the previous pose crosses the region's face, in ticks.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Crossing {
    pub tick: usize,
    pub secant: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Contact {
    pub tick: usize,
    pub horizontal: bool,
    /// A vertical collision that isn't standing on ground (a ceiling).
    pub ceiling: bool,
    pub fall_flying: bool,
    /// Vanilla's fly-into-wall damage, `10 (h_before - h_after) - 3` as a float when
    /// positive, judged on the client's copy (the server's should agree).
    pub damage: f32,
    pub before_finish: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroundTouch {
    pub tick: usize,
    pub ticks: usize,
    pub pos: [f64; 3],
    pub timer_running: bool,
    pub allowed: bool,
}

/// A planned perfect bounce: a gliding landing in the ground-allowed region.
#[derive(Debug, Clone, Serialize)]
pub struct Bounce {
    /// The landing tick: its move ends on the ground.
    pub landing: usize,
    /// The tick that jumped off (the first tick back in the air), if any.
    pub jump: Option<usize>,
    pub pos: [f64; 3],
    /// Arrival ticks of the glide-off, in `landing + GLIDE_OFF_MIN ..= landing +
    /// GLIDE_OFF_MAX` and before the next bounce's window could start, at which a
    /// jump press flies the plan's poses.
    pub rescuable: Vec<usize>,
    /// Those that don't, and why.
    pub not_rescuable: Vec<(usize, String)>,
    /// The plan's `@react` window for it, if the plan has one.
    pub window: Option<(usize, usize)>,
}

impl Bounce {
    /// The window a plan should carry: the first run of consecutive rescuable ticks.
    pub fn window_to_plan(&self) -> Option<(usize, usize)> {
        let t0 = *self.rescuable.first()?;
        let mut t1 = t0;
        while self.rescuable.contains(&(t1 + 1)) {
            t1 += 1;
        }
        Some((t0, t1))
    }

    /// The glide-off delays (ticks after the landing) the plan's window covers.
    pub fn covered(&self) -> Option<(usize, usize)> {
        self.window
            .map(|(a, b)| (a - self.landing, b - self.landing))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KillHit {
    pub rule: String,
    pub tick: usize,
    pub pos: [f64; 3],
}

#[derive(Debug, Clone, Serialize)]
pub struct Clearance {
    /// Smallest gap between the player's box and a collision box, over airborne ticks
    /// without collision between the timer start and the finish (whole run if none).
    pub min: f64,
    pub tick: usize,
    /// Ticks where unknown space was within `RADIUS`: their clearance is a bound.
    pub unknown_near: usize,
    pub ticks: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Roughness {
    pub ticks: usize,
    pub pitch_d2_rms: f64,
    pub pitch_d2_max: f64,
    pub yaw_d2_rms: f64,
    pub yaw_d2_max: f64,
    /// Sign reversals of the per-tick change, both changes over `CHATTER_DEG`.
    pub pitch_reversals: usize,
    pub yaw_reversals: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub ticks: usize,
    pub starts_at_course_start: bool,
    /// The first tick whose recorded state port doesn't reproduce, and how.
    pub mismatch: Option<(usize, String)>,
    /// A tick port stopped at (unknown space, killing block, unsupported block).
    pub stopped: Option<(usize, String)>,
    /// Per variant (working, inner, outer).
    pub timer: Vec<(String, Option<Crossing>)>,
    pub finish: Vec<(String, Option<Crossing>)>,
    pub chat_lag: [usize; 2],
    pub contacts: Vec<Contact>,
    pub ground: Vec<GroundTouch>,
    pub bounces: Vec<Bounce>,
    pub kills: Vec<KillHit>,
    pub clearance: Option<Clearance>,
    pub roughness: Roughness,
    pub provisional: Vec<String>,
    pub failures: Vec<String>,
}

pub const RADIUS: f64 = 2.0;
pub const CHATTER_DEG: f64 = 0.01;

impl Report {
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }

    fn get(v: &[(String, Option<Crossing>)], name: &str) -> Option<Crossing> {
        v.iter().find(|(n, _)| n == name).and_then(|(_, c)| *c)
    }

    pub fn timer_tick(&self) -> Option<usize> {
        Self::get(&self.timer, "working").map(|c| c.tick)
    }

    pub fn finish_tick(&self) -> Option<usize> {
        Self::get(&self.finish, "working").map(|c| c.tick)
    }

    /// EMC's timer as predicted: whole server ticks between the two judged poses.
    pub fn timer_seconds(&self) -> Option<f64> {
        Some((self.finish_tick()? - self.timer_tick()?) as f64 * 0.05)
    }

    pub fn secant_seconds(&self) -> Option<f64> {
        let (s, f) = (
            Self::get(&self.timer, "working")?,
            Self::get(&self.finish, "working")?,
        );
        Some((f.secant - s.secant) * 0.05)
    }
}

fn unsupported(u: Unsupported) -> String {
    let p = |b: port::BlockPos| format!("{} {} {}", b.x, b.y, b.z);
    match u {
        Unsupported::UnknownSpace(b) => format!("entered unknown space at {}", p(b)),
        Unsupported::Climbable(b) => {
            format!("killing block (climbable: ends the glide) at {}", p(b))
        }
        Unsupported::Fluid(b) => format!("fluid at {} (not simulated)", p(b)),
        Unsupported::Stuck(b) => format!("stuck-in block at {} (not simulated)", p(b)),
        Unsupported::Bounce(b) => format!("bouncing block at {} (not simulated)", p(b)),
    }
}

fn box_gap(a: &Aabb, b: &Aabb) -> f64 {
    let d: Vec<f64> = (0..3)
        .map(|k| (b.min[k] - a.max[k]).max(a.min[k] - b.max[k]).max(0.0))
        .collect();
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn horizontal(v: port::Vec3) -> f64 {
    (v.x * v.x + v.z * v.z).sqrt()
}

/// Replay `plan` under `course` and `world`. Never panics on bad plans; everything
/// wrong is in the report's `failures`.
// ticks index several parallel per-tick vectors
#[allow(clippy::needless_range_loop)]
pub fn verify(course: &Course, world: &World, plan: Plan) -> Report {
    let reacts: Option<&[React]> = match &plan {
        Plan::Trajectory(_) => None,
        Plan::Tape(_) => Some(&[]),
        Plan::Reactive(p) => Some(&p.reacts),
    };
    let plan = match plan {
        Plan::Reactive(p) => Plan::Tape(&p.tape),
        other => other,
    };
    // the start, the inputs, and the recorded states to hold them to
    let (mut p, inputs, rec_full, rec_tape): (
        LocalPlayer,
        Vec<TickInput>,
        Option<Vec<&FullState>>,
        Option<&Tape>,
    ) = match plan {
        Plan::Trajectory(t) => {
            let mut inputs = Vec::new();
            for r in &t.rows {
                match r.input.tick_input() {
                    Ok(i) => inputs.push(i),
                    Err(e) => {
                        return early(course, format!("tick {}: input: {e}", r.t));
                    }
                }
            }
            match t.initial.to_player() {
                Ok(p) => (
                    p,
                    inputs,
                    Some(t.rows.iter().map(|r| &r.state).collect()),
                    None,
                ),
                Err(e) => return early(course, format!("initial state: {e}")),
            }
        }
        Plan::Tape(tape) => {
            let mut inputs = Vec::new();
            for r in &tape.rows {
                match crate::state::keys_of(&r.keys) {
                    Ok(keys) => inputs.push(TickInput {
                        y_rot: r.yaw,
                        x_rot: r.pitch,
                        keys,
                    }),
                    Err(e) => return early(course, format!("tick {}: keys: {e}", r.t)),
                }
            }
            (
                rest_state(tape.start_pos, tape.start_yaw, tape.start_pitch),
                inputs,
                None,
                Some(tape),
            )
        }
        Plan::Reactive(_) => unreachable!("turned into its tape above"),
    };
    let start = course.start_state().ok();
    let starts_at_course_start = start
        .as_ref()
        .is_some_and(|s| s.position == p.position && s.y_rot == p.y_rot && s.x_rot == p.x_rot);

    let mut players = vec![p.clone()];
    let mut mismatch = None;
    let mut stopped = None;
    for (i, input) in inputs.iter().enumerate() {
        let t = i + 1;
        if let Err(u) = tick(&mut p, input, world) {
            stopped = Some((t, unsupported(u)));
            break;
        }
        if mismatch.is_none() {
            let got = FullState::from(&p);
            if let Some(rec) = &rec_full {
                if *rec[i] != got {
                    mismatch = Some((t, got.first_difference(rec[i]).unwrap_or_default()));
                }
            } else if let Some(tape) = rec_tape {
                let r = &tape.rows[i];
                if r.pos != got.position || r.ground != got.on_ground {
                    mismatch = Some((
                        t,
                        format!(
                            "pose {:?} ground {} vs tape {:?} ground {}",
                            got.position, got.on_ground, r.pos, r.ground
                        ),
                    ));
                }
            }
        }
        players.push(p.clone());
    }
    let n = players.len() - 1;
    let obs: Vec<Observed> = players
        .iter()
        .map(|q| Observed {
            pos: [q.position.x, q.position.y, q.position.z],
            on_ground: q.on_ground,
            fall_flying: q.fall_flying,
        })
        .collect();

    // rules
    let first = |rule: &Rule, from: usize, v: Variant, enter: bool| -> Option<Crossing> {
        let t = (from..=n).find(|&t| course.eval(&rule.when, &obs[t], v))?;
        let region = region_of(&rule.when);
        let s = region
            .and_then(|r| course.regions.get(r))
            .and_then(|r| r.bounds(v).crossing(obs[t - 1].pos, obs[t].pos, enter))
            .unwrap_or(1.0);
        Some(Crossing {
            tick: t,
            secant: (t - 1) as f64 + s,
        })
    };
    let mut timer = Vec::new();
    let mut finish = Vec::new();
    for v in Variant::ALL {
        let ts = first(&course.rules.timer_start, 1, v, false);
        let tf = ts.and_then(|ts| first(&course.rules.finish, ts.tick + 1, v, true));
        timer.push((v.name().to_string(), ts));
        finish.push((v.name().to_string(), tf));
    }
    let ts = timer[0].1.map(|c| c.tick);
    let tf = finish[0].1.map(|c| c.tick);
    let running = |t: usize| ts.is_some_and(|s| t >= s) && tf.is_none_or(|f| t < f);
    let bound = |name: &str, default: usize| match name {
        "start" => 1,
        "timer_start" => ts.unwrap_or(n + 1),
        "finish" => tf.unwrap_or(n + 1),
        _ => default,
    };
    let mut kills = Vec::new();
    for rule in &course.rules.kill {
        let (a, b) = (
            bound(&rule.after, 1),
            rule.until.as_deref().map_or(n + 1, |u| bound(u, n + 1)),
        );
        for t in a..b.min(n + 1) {
            if course.eval(&rule.when, &obs[t], Variant::Working) {
                kills.push(KillHit {
                    rule: rule.name.clone().unwrap_or_else(|| "kill".into()),
                    tick: t,
                    pos: obs[t].pos,
                });
                // a kill ends the attempt, so later hits say nothing ("any" and
                // "first" differ only for rules that don't kill)
                break;
            }
        }
    }

    // ground touches, contacts
    let mut ground = Vec::new();
    let mut t = 1;
    while t <= n {
        if obs[t].on_ground {
            let a = t;
            while t <= n && obs[t].on_ground {
                t += 1;
            }
            ground.push(GroundTouch {
                tick: a,
                ticks: t - a,
                pos: obs[a].pos,
                timer_running: running(a),
                allowed: !running(a) || course.ground_allowed(obs[a].pos, Variant::Working),
            });
        } else {
            t += 1;
        }
    }
    let mut contacts = Vec::new();
    for t in 1..=n {
        let (b, a) = (&players[t - 1], &players[t]);
        let ceiling = a.vertical_collision && !a.vertical_collision_below;
        if !a.horizontal_collision && !ceiling {
            continue;
        }
        let mut damage = 0.0;
        if b.fall_flying && a.horizontal_collision {
            let d =
                ((horizontal(b.delta_movement) - horizontal(a.delta_movement)) * 10.0 - 3.0) as f32;
            if d > 0.0 {
                damage = d;
            }
        }
        contacts.push(Contact {
            tick: t,
            horizontal: a.horizontal_collision,
            ceiling,
            fall_flying: b.fall_flying,
            damage,
            before_finish: tf.is_none_or(|f| t < f),
        });
    }

    // planned perfect bounces, and gliding landings elsewhere
    let mut bounces = Vec::new();
    let mut bad_landings = Vec::new();
    let landings: Vec<usize> = (1..=n)
        .filter(|&t| {
            players[t].on_ground
                && !players[t - 1].on_ground
                && players[t].fall_flying
                && tf.is_none_or(|f| t < f)
        })
        .collect();
    for (i, &l) in landings.iter().enumerate() {
        if !course.ground_allowed(obs[l].pos, Variant::Working) {
            bad_landings.push(l);
            continue;
        }
        let next = landings.get(i + 1).copied();
        let jump = (l + 1..=n).find(|&t| !players[t].on_ground);
        // arrivals after the next landing are the next bounce's (its window starts
        // at least GLIDE_OFF_MIN after it)
        let last = (l + GLIDE_OFF_MAX).min(n).min(next.unwrap_or(usize::MAX));
        let mut rescuable = Vec::new();
        let mut not_rescuable = Vec::new();
        for t in l + GLIDE_OFF_MIN..=last {
            match rescue(world, &players, &inputs, l, t) {
                Ok(()) => rescuable.push(t),
                Err(why) => not_rescuable.push((t, why)),
            }
        }
        bounces.push(Bounce {
            landing: l,
            jump,
            pos: obs[l].pos,
            rescuable,
            not_rescuable,
            window: None,
        });
    }
    let mut react_failures = Vec::new();
    if let Some(reacts) = reacts {
        let mut used = vec![false; reacts.len()];
        for b in bounces.iter_mut() {
            let found = reacts.iter().enumerate().find(|(_, r)| {
                r.event == Event::GlideOff
                    && r.action == Action::PressJump
                    && r.t0 > b.landing
                    && r.t0 <= b.landing + GLIDE_OFF_MAX
            });
            match found {
                None => react_failures.push(format!(
                    "bounce at tick {}: no @react glide_off press_jump within {} ticks after it",
                    b.landing, GLIDE_OFF_MAX
                )),
                Some((j, r)) => {
                    used[j] = true;
                    b.window = Some((r.t0, r.t1));
                    for t in r.t0..=r.t1 {
                        if b.rescuable.contains(&t) {
                            continue;
                        }
                        let why = match b.not_rescuable.iter().find(|(u, _)| *u == t) {
                            Some((_, w)) => w.clone(),
                            None => rescue(world, &players, &inputs, b.landing, t)
                                .err()
                                .unwrap_or_default(),
                        };
                        if !why.is_empty() {
                            react_failures.push(format!(
                                "bounce at tick {}: a glide-off at tick {t} in its window doesn't fly as planned ({why})",
                                b.landing
                            ));
                        }
                    }
                    for d in GLIDE_OFF_NATURAL {
                        if !(r.t0..=r.t1).contains(&(b.landing + d)) {
                            react_failures.push(format!(
                                "bounce at tick {}: window {}..{} misses a glide-off {d} ticks after the landing",
                                b.landing, r.t0, r.t1
                            ));
                        }
                    }
                }
            }
        }
        for (j, r) in reacts.iter().enumerate() {
            if !used[j] {
                react_failures.push(format!("{} covers no planned bounce", r.line()));
            }
            if let Some(q) = reacts[..j].iter().find(|q| q.t0 <= r.t1 && r.t0 <= q.t1) {
                react_failures.push(format!("{} overlaps {}", r.line(), q.line()));
            }
        }
    }

    // clearance
    let (c0, c1) = match (ts, tf) {
        (Some(s), Some(f)) => (s, f),
        _ => (1, n),
    };
    let mut clearance: Option<Clearance> = None;
    let mut unknown_near = 0;
    let mut counted = 0;
    for t in c0..=c1.min(n) {
        let q = &players[t];
        if q.on_ground || q.horizontal_collision || q.vertical_collision {
            continue;
        }
        let bb = q.get_bounding_box();
        let me = Aabb {
            min: [bb.min_x, bb.min_y, bb.min_z],
            max: [bb.max_x, bb.max_y, bb.max_z],
        };
        let area = Aabb {
            min: me.min.map(|v| v - RADIUS),
            max: me.max.map(|v| v + RADIUS),
        };
        counted += 1;
        let gap = match world.collision_boxes(area) {
            Ok(boxes) => boxes.iter().map(|b| box_gap(&me, b)).fold(RADIUS, f64::min),
            Err(_) => {
                unknown_near += 1;
                RADIUS
            }
        };
        if clearance.as_ref().is_none_or(|c| gap < c.min) {
            clearance = Some(Clearance {
                min: gap,
                tick: t,
                unknown_near: 0,
                ticks: 0,
            });
        }
    }
    if let Some(c) = clearance.as_mut() {
        c.unknown_near = unknown_near;
        c.ticks = counted;
    }

    // roughness over gliding ticks
    let glide: Vec<usize> = (1..=n).filter(|&t| players[t].fall_flying).collect();
    let pitch: Vec<f64> = glide.iter().map(|&t| inputs[t - 1].x_rot as f64).collect();
    let yaw: Vec<f64> = glide.iter().map(|&t| inputs[t - 1].y_rot as f64).collect();
    let rough = |a: &[f64]| -> (f64, f64, usize) {
        if a.len() < 3 {
            return (0.0, 0.0, 0);
        }
        let d2: Vec<f64> = a.windows(3).map(|w| w[2] - 2.0 * w[1] + w[0]).collect();
        let rms = (d2.iter().map(|x| x * x).sum::<f64>() / d2.len() as f64).sqrt();
        let max = d2.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        let d1: Vec<f64> = a.windows(2).map(|w| w[1] - w[0]).collect();
        let rev = d1
            .windows(2)
            .filter(|w| {
                w[0].abs() > CHATTER_DEG
                    && w[1].abs() > CHATTER_DEG
                    && w[0].signum() != w[1].signum()
            })
            .count();
        (rms, max, rev)
    };
    let (pr, pm, pv) = rough(&pitch);
    let (yr, ym, yv) = rough(&yaw);
    let roughness = Roughness {
        ticks: glide.len(),
        pitch_d2_rms: pr,
        pitch_d2_max: pm,
        yaw_d2_rms: yr,
        yaw_d2_max: ym,
        pitch_reversals: pv,
        yaw_reversals: yv,
    };

    let mut provisional: Vec<String> = Vec::new();
    for (name, r) in &course.regions {
        if r.status != "fitted" {
            let assumed: Vec<&str> = r
                .faces
                .iter()
                .filter(|(_, v)| v.as_str() != "fitted")
                .map(|(k, _)| k.as_str())
                .collect();
            provisional.push(format!(
                "region {name}: assumed faces {}",
                assumed.join(" ")
            ));
        }
    }
    for r in &course.rules.kill {
        if r.status != "fitted" {
            provisional.push(format!(
                "kill rule {}: {}",
                r.name.as_deref().unwrap_or("?"),
                r.status
            ));
        }
    }

    let mut failures = Vec::new();
    if let Some((t, m)) = &mismatch {
        failures.push(format!("tick {t}: recorded state doesn't reproduce ({m})"));
    }
    if let Some((t, m)) = &stopped {
        failures.push(format!("tick {t}: {m}"));
    }
    if ts.is_none() {
        failures.push("the timer never starts (working start area)".into());
    } else if tf.is_none() {
        failures.push("never reaches the finish (working box)".into());
    }
    for l in &bad_landings {
        failures.push(format!(
            "tick {l}: gliding landing outside the ground-allowed region at {:?} (the glide ends there)",
            obs[*l].pos
        ));
    }
    failures.extend(react_failures);
    for k in &kills {
        failures.push(format!(
            "tick {}: kill rule {} at {:?}",
            k.tick, k.rule, k.pos
        ));
    }
    for c in contacts
        .iter()
        .filter(|c| c.before_finish && c.damage > 0.0)
    {
        failures.push(format!(
            "tick {}: wall hit, predicted damage {:.2}",
            c.tick, c.damage
        ));
    }

    Report {
        ticks: n,
        starts_at_course_start,
        mismatch,
        stopped,
        timer,
        finish,
        chat_lag: course.observation.tick_chat_lag,
        contacts,
        ground,
        bounces,
        kills,
        clearance,
        roughness,
        provisional,
        failures,
    }
}

/// The pose the server's glide-off carries: what its `updatePlayerPose` picks for a
/// player on the ground at the landing position without the glide (standing if it
/// fits, else crouching, else swimming; the client takes it with the flag).
fn server_pose(world: &World, landed: &LocalPlayer) -> port::Pose {
    let fits = |pose: port::Pose| {
        port::level::no_collision(
            world,
            &pose.make_bounding_box(landed.position).deflate(1.0E-7),
        )
        .unwrap_or(false)
    };
    if !fits(port::Pose::Swimming) {
        return landed.pose;
    }
    if fits(port::Pose::Standing) {
        port::Pose::Standing
    } else if fits(port::Pose::Crouching) {
        port::Pose::Crouching
    } else {
        port::Pose::Swimming
    }
}

/// Whether a glide-off first handled before tick `t` (of the bounce landing at
/// `landing`), answered with jump held for that tick, flies the plan: from the state
/// after tick `t - 1` with the flag off and the server's pose, port flies the plan's
/// inputs (jump added at `t`) to the plan's poses, until its full state is the plan's
/// again or the plan ends. A press can't restart the glide on the ground
/// (`canGlide`), nor when the tape already held jump the tick before (no edge); and a
/// tick without the glide may start a sprint (`canStartSprinting`), which the plan's
/// next jump would feel.
fn rescue(
    world: &World,
    players: &[LocalPlayer],
    inputs: &[TickInput],
    landing: usize,
    t: usize,
) -> Result<(), String> {
    let base = &players[t - 1];
    if !base.fall_flying {
        return Err("not gliding before it".into());
    }
    if base.on_ground {
        return Err("on the ground: the press can't restart the glide".into());
    }
    if base.input.key_presses.jump {
        return Err(format!("jump held at tick {}: the press is no edge", t - 1));
    }
    let mut q = base.clone();
    q.fall_flying = false;
    q.pose = server_pose(world, &players[landing]);
    let mut sprint_started = false;
    for u in t..players.len() {
        let mut input = inputs[u - 1];
        if u == t {
            input.keys.jump = true;
        }
        tick(&mut q, &input, world).map_err(|e| format!("tick {u}: {}", unsupported(e)))?;
        let want = &players[u];
        if u == t {
            sprint_started = q.sprinting && !want.sprinting;
        }
        if q.position != want.position || q.on_ground != want.on_ground {
            let d = q
                .position
                .subtract_xyz(want.position.x, want.position.y, want.position.z);
            return Err(format!(
                "tick {u}: off by {:.2e}{}{}",
                d.length_sqr().sqrt(),
                if q.fall_flying { "" } else { ", not gliding" },
                if sprint_started {
                    format!("; the tick without the glide started a sprint at {t}")
                } else {
                    String::new()
                }
            ));
        }
        if FullState::from(&q) == FullState::from(want) {
            return Ok(());
        }
    }
    Ok(())
}

fn region_of(p: &crate::course::Pred) -> Option<&str> {
    use crate::course::Pred;
    match p {
        Pred::In(r) => Some(r),
        Pred::Not(q) => region_of(q),
        Pred::And(v) | Pred::Or(v) => v.iter().find_map(region_of),
        Pred::Field(_) => None,
    }
}

fn early(course: &Course, why: String) -> Report {
    Report {
        ticks: 0,
        starts_at_course_start: false,
        mismatch: None,
        stopped: None,
        timer: vec![],
        finish: vec![],
        chat_lag: course.observation.tick_chat_lag,
        contacts: vec![],
        ground: vec![],
        bounces: vec![],
        kills: vec![],
        clearance: None,
        roughness: Roughness::default(),
        provisional: vec![],
        failures: vec![why],
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [l0, l1] = self.chat_lag;
        writeln!(
            f,
            "{} ticks{}",
            self.ticks,
            if self.starts_at_course_start {
                ""
            } else {
                " (not from the course start)"
            }
        )?;
        match &self.mismatch {
            None => writeln!(f, "recorded states: reproduced")?,
            Some((t, m)) => writeln!(f, "recorded states: tick {t} differs: {m}")?,
        }
        if let Some((t, m)) = &self.stopped {
            writeln!(f, "stopped at tick {t}: {m}")?;
        }
        let show = |f: &mut fmt::Formatter<'_>,
                    what: &str,
                    v: &[(String, Option<Crossing>)]|
         -> fmt::Result {
            let parts: Vec<String> = v
                .iter()
                .map(|(n, c)| match c {
                    Some(c) => format!("{n} {} (secant {:.3})", c.tick, c.secant),
                    None => format!("{n} -"),
                })
                .collect();
            writeln!(f, "{what} judged at tick: {}", parts.join(", "))
        };
        show(f, "timer start", &self.timer)?;
        show(f, "finish", &self.finish)?;
        if let (Some(s), Some(e)) = (self.timer_tick(), self.finish_tick()) {
            writeln!(
                f,
                "predicted EMC chat ticks: timer {}..{}, finish {}..{}",
                s + l0,
                s + l1,
                e + l0,
                e + l1
            )?;
            writeln!(
                f,
                "predicted EMC timer: {} ticks = {:.2} s (shown within 0.02 s of it); secant time {:.3} s",
                e - s,
                self.timer_seconds().unwrap_or(0.0),
                self.secant_seconds().unwrap_or(0.0)
            )?;
        }
        let dmg: Vec<String> = self
            .contacts
            .iter()
            .map(|c| {
                format!(
                    "{}{}{}{}",
                    c.tick,
                    if c.horizontal { " wall" } else { " ceiling" },
                    if c.damage > 0.0 {
                        format!(" damage {:.2}", c.damage)
                    } else {
                        String::new()
                    },
                    if c.before_finish {
                        ""
                    } else {
                        " (after finish)"
                    }
                )
            })
            .collect();
        writeln!(
            f,
            "contacts: {}",
            if dmg.is_empty() {
                "none".into()
            } else {
                dmg.join(", ")
            }
        )?;
        let g: Vec<String> = self
            .ground
            .iter()
            .map(|g| {
                format!(
                    "{}+{} {}",
                    g.tick,
                    g.ticks,
                    if !g.timer_running {
                        "(timer not running)"
                    } else if g.allowed {
                        "allowed"
                    } else {
                        "NOT allowed"
                    }
                )
            })
            .collect();
        writeln!(
            f,
            "ground touches: {}",
            if g.is_empty() {
                "none".into()
            } else {
                g.join(", ")
            }
        )?;
        for b in &self.bounces {
            let delays = |ts: &mut dyn Iterator<Item = usize>| -> String {
                let v: Vec<String> = ts.map(|t| (t - b.landing).to_string()).collect();
                if v.is_empty() {
                    "none".into()
                } else {
                    v.join(" ")
                }
            };
            writeln!(
                f,
                "bounce at tick {}{}: {}; glide-off delays a press rescues: {} (of {GLIDE_OFF_MIN}..{GLIDE_OFF_MAX}, or up to the next landing)",
                b.landing,
                match b.jump {
                    Some(j) => format!(", jump {j}"),
                    None => ", no jump".into(),
                },
                match (b.window, b.covered()) {
                    (Some((t0, t1)), Some((d0, d1))) =>
                        format!("@react {t0} {t1} covers delays {d0}..{d1}"),
                    _ => "no @react".into(),
                },
                delays(&mut b.rescuable.iter().copied()),
            )?;
        }
        if let Some(c) = &self.clearance {
            writeln!(
                f,
                "clearance: min {:.3} at tick {} over {} airborne ticks (radius {RADIUS}{})",
                c.min,
                c.tick,
                c.ticks,
                if c.unknown_near > 0 {
                    format!("; unknown space near on {} ticks", c.unknown_near)
                } else {
                    String::new()
                }
            )?;
        }
        let r = &self.roughness;
        writeln!(
            f,
            "roughness over {} gliding ticks: pitch d2 rms {:.3} max {:.3} deg, yaw d2 rms {:.3} max {:.3} deg; reversals pitch {} yaw {}",
            r.ticks,
            r.pitch_d2_rms,
            r.pitch_d2_max,
            r.yaw_d2_rms,
            r.yaw_d2_max,
            r.pitch_reversals,
            r.yaw_reversals
        )?;
        for p in &self.provisional {
            writeln!(f, "provisional: {p}")?;
        }
        if self.failures.is_empty() {
            writeln!(f, "PASS")
        } else {
            for x in &self.failures {
                writeln!(f, "FAIL: {x}")?;
            }
            Ok(())
        }
    }
}
