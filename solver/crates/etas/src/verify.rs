//! `verify`: replay a trajectory or a `.tas` plan with port against a course's world
//! and rules, and say whether it holds. The judge is the least clever code we have
//! (ARCHITECTURE.md, Verify): port, the world file, and the course's predicates over
//! each tick's end pose, nothing from the solver that made the plan.

use std::fmt;

use port::{LocalPlayer, TickInput, Unsupported, tick};
use serde::Serialize;
use world::{Aabb, World};

use crate::course::{Course, Observed, Rule, Variant, rest_state};
use crate::state::FullState;
use crate::tape::Tape;
use crate::trajectory::Trajectory;

/// What verify replays.
pub enum Plan<'a> {
    Trajectory(&'a Trajectory),
    Tape(&'a Tape),
}

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
        kills,
        clearance,
        roughness,
        provisional,
        failures,
    }
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
