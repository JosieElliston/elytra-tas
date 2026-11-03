//! Re-fly a generic solution under the faithful kernel and report where it parts
//! (ARCHITECTURE.md, Tools: transforms), and the repair: re-solve from the flown state
//! as it goes.
//!
//! The repair is corner-time's finding that re-solving is the feedback: the DDP's
//! linear gains are useless next to an active wall, while a warm re-solve from the
//! flown state (the last plan and its multipliers shifted by a tick, the price started
//! stiff) costs a few iterations and closes the gap.

use std::time::Instant;

use sim::faithful::{Event, Faithful};
use sim::state::{FlightState, Input};
use world::World;

use super::ddp::{Goal, Multipliers, Plan, Problem, Solved, X, input_at};

fn fs(x: &X) -> FlightState<f64> {
    FlightState {
        pos: [x[0], x[1], x[2]],
        vel: [x[3], x[4], x[5]],
    }
}

fn xs(s: &FlightState<f64>) -> X {
    [s.pos[0], s.pos[1], s.pos[2], s.vel[0], s.vel[1], s.vel[2]]
}

fn dist(a: &X, b: &X) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Where a faithful flight leaves the generic one it was planned from.
#[derive(Debug, Clone, PartialEq)]
pub struct Parting {
    /// The faithful states, `x0` first, as far as the flight got.
    pub flown: Vec<X>,
    /// Per tick, the position gap to the generic state of the same tick.
    pub gaps: Vec<f64>,
    /// The first tick (1-based) whose gap is over the tolerance asked for.
    pub first_over: Option<usize>,
    /// Ticks with a wall or ceiling contact.
    pub contacts: Vec<usize>,
    /// The event that ended normal flight, and its tick.
    pub event: Option<(usize, Event)>,
}

impl Parting {
    pub fn max_gap(&self) -> f64 {
        self.gaps.iter().copied().fold(0.0, f64::max)
    }

    /// Normal flight throughout, without contact.
    pub fn clean(&self) -> bool {
        self.contacts.is_empty() && self.event.is_none()
    }
}

/// Fly `inputs` from `x0` under the faithful kernel, against `generic`, the states
/// the plan predicted (`generic[0] == x0`).
pub fn refly(world: &World, x0: &X, generic: &[X], inputs: &[Input<f32>], tol: f64) -> Parting {
    let f = Faithful;
    let mut p = Parting {
        flown: vec![*x0],
        gaps: Vec::new(),
        first_over: None,
        contacts: Vec::new(),
        event: None,
    };
    for (k, u) in inputs.iter().enumerate() {
        let s = fs(p.flown.last().unwrap());
        match f.step_input(world, &s, u) {
            Ok(t) => {
                let x = xs(&t.state);
                let gap = generic.get(k + 1).map_or(f64::NAN, |g| dist(&x, g));
                if p.first_over.is_none() && (gap.is_nan() || gap > tol) {
                    p.first_over = Some(k + 1);
                }
                if t.contact.is_some() {
                    p.contacts.push(k + 1);
                }
                p.gaps.push(gap);
                p.flown.push(x);
            }
            Err(e) => {
                p.event = Some((k + 1, e));
                break;
            }
        }
    }
    p
}

/// What a repaired flight came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Repaired {
    /// The rotations flown, and the faithful states (`x0` first).
    pub inputs: Vec<Input<f32>>,
    pub flown: Vec<X>,
    pub contacts: Vec<usize>,
    pub event: Option<(usize, Event)>,
    /// Re-solves run, and the seconds they took in all.
    pub resolves: usize,
    pub secs: f64,
}

impl Repaired {
    pub fn clean(&self) -> bool {
        self.contacts.is_empty() && self.event.is_none()
    }
}

/// Receding horizon under the faithful kernel. Each tick, fly the first control of the
/// current plan from the faithful state: the lead is taken off the flown velocity, so
/// that much feedback is free. Before it, re-solve the rest from the flown state
/// (warm: the plan and multipliers shifted, the price at `c0`) unless the plan still
/// meets the goal and the margin from there. A Reach goal stops re-solving when
/// `stop_last` ticks are left: fewer ticks than that can't hold all six coordinates.
/// A Plane goal ends at the first faithful tick past the plane.
pub fn repair(
    world: &World,
    pb: &Problem,
    solved: &Solved,
    yaw_from: f64,
    c0: f64,
    stop_last: usize,
) -> Repaired {
    let f = Faithful;
    let mut plan: Plan = solved.plan.clone();
    let mut mult: Multipliers = solved.mult.clone();
    let mut out = Repaired {
        inputs: Vec::new(),
        flown: vec![pb.x0],
        contacts: Vec::new(),
        event: None,
        resolves: 0,
        secs: 0.0,
    };
    let mut yaw_prev = yaw_from;
    let cap = match pb.goal {
        Goal::Plane { cap, .. } => cap,
        Goal::Reach { .. } => solved.plan.u.len(),
    };
    for k in 0..cap {
        let x = *out.flown.last().unwrap();
        if plan.u.is_empty() {
            match pb.goal {
                Goal::Reach { .. } => break,
                Goal::Plane { .. } => plan.u.push(*solved.plan.u.last().unwrap()),
            }
        }
        let sub = Problem {
            x0: x,
            ..pb.clone()
        };
        let left = plan.u.len();
        let reach = matches!(pb.goal, Goal::Reach { .. });
        if !reach || left > stop_last {
            let mut check = mult.clone();
            let xr = sub.refit(&mut check, &mut plan.u);
            let ok = sub.feasible(&Plan {
                u: plan.u.clone(),
                x: xr,
            });
            if !ok {
                let t = Instant::now();
                let mut m = mult.clone();
                m.rho = c0;
                let s = sub.solve_from(m, &plan.u);
                out.secs += t.elapsed().as_secs_f64();
                out.resolves += 1;
                plan = s.plan;
                mult = s.mult;
            }
        }
        let (input, yaw) = input_at(&x, &plan.u[0], yaw_prev);
        yaw_prev = yaw;
        out.inputs.push(input);
        match f.step_input(world, &fs(&x), &input) {
            Ok(t) => {
                if t.contact.is_some() {
                    out.contacts.push(k + 1);
                }
                let nx = xs(&t.state);
                out.flown.push(nx);
                if let Goal::Plane { normal: n, at, .. } = pb.goal
                    && nx[0] * n[0] + nx[1] * n[1] + nx[2] * n[2] - at > 0.0
                {
                    break;
                }
            }
            Err(e) => {
                out.event = Some((k + 1, e));
                break;
            }
        }
        plan.u.remove(0);
        plan.x.remove(0);
        mult = mult.shifted(1);
    }
    out
}
