//! Re-solving a stretch of a recorded trajectory: the problem's pieces taken from the
//! recording, and the splice of a new stretch back into it.
//!
//! Ticks are the trajectory's: the state "at tick t" is row `t`'s (the end of tick
//! `t`), the initial state at 0. A stretch `a..b` starts from the state at `a` and
//! replaces the inputs of ticks `a + 1 ..= b`.

use port::{TickInput, Unsupported, tick};
use world::World;

use super::ddp::{U, X, control, lead_of};
use crate::state::{FullState, keys_of};
use crate::trajectory::{Header, InputRow, Row, Trajectory};

/// A stretch of a recorded run, as a solver sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Stretch {
    pub a: usize,
    pub b: usize,
    /// The flight states at `a` and at `b`.
    pub x0: X,
    pub target: X,
    /// The recorded rotation at `a`: where the plan's yaw branch starts.
    pub yaw_from: f64,
    /// The recorded inputs as controls, `b - a` of them.
    pub seed: Vec<U>,
    /// The recorded positions `a ..= b`.
    pub path: Vec<[f64; 3]>,
}

fn state_at(rec: &Trajectory, t: usize) -> &FullState {
    if t == 0 {
        &rec.initial
    } else {
        &rec.rows[t - 1].state
    }
}

fn flight(s: &FullState) -> X {
    let (p, v) = (s.position, s.delta_movement);
    [p[0], p[1], p[2], v[0], v[1], v[2]]
}

/// The stretch `a..b` of `rec`. Fails unless every state in it is contact-free normal
/// flight: gliding, off the ground, in the gliding pose, no collision flags.
pub fn stretch(rec: &Trajectory, a: usize, b: usize) -> Result<Stretch, String> {
    if !(a < b && b <= rec.rows.len()) {
        return Err(format!("stretch {a}..{b} of {} ticks", rec.rows.len()));
    }
    for t in a..=b {
        let s = state_at(rec, t);
        if !(s.fall_flying && !s.on_ground && s.pose == "fall_flying")
            || s.horizontal_collision
            || s.vertical_collision
        {
            return Err(format!("tick {t} is not contact-free normal flight"));
        }
    }
    let seed = (a..b)
        .map(|t| {
            let i = &rec.rows[t].input;
            control(lead_of(&flight(state_at(rec, t)), i.yaw), i.pitch)
        })
        .collect();
    Ok(Stretch {
        a,
        b,
        x0: flight(state_at(rec, a)),
        target: flight(state_at(rec, b)),
        yaw_from: state_at(rec, a).y_rot,
        seed,
        path: (a..=b).map(|t| state_at(rec, t).position).collect(),
    })
}

/// `u` stretched or squeezed to `n` ticks, linearly in tick fraction.
pub fn resample(u: &[U], n: usize) -> Vec<U> {
    if u.len() == n || u.is_empty() {
        return u.to_vec();
    }
    (0..n)
        .map(|j| {
            let s = if n == 1 {
                0.0
            } else {
                j as f64 * (u.len() - 1) as f64 / (n - 1) as f64
            };
            let (i, w) = (s.floor() as usize, s - s.floor());
            let k = (i + 1).min(u.len() - 1);
            std::array::from_fn(|c| u[i][c] + w * (u[k][c] - u[i][c]))
        })
        .collect()
}

/// `rec` with the inputs of ticks `a + 1 ..= b` replaced by `rotations`, flown with
/// port from `rec`'s initial state: the prefix and the recorded suffix are replayed as
/// recorded, so whatever the new stretch changed carries into the suffix. The new
/// ticks keep the keys of the last `rotations.len()` recorded ticks of the stretch, so
/// the keys after the stretch are the recording's. Returns the trajectory as far as
/// port got, and the error that stopped it.
pub fn splice(
    rec: &Trajectory,
    a: usize,
    b: usize,
    rotations: &[sim::state::Input<f32>],
    world: &World,
    header: Header,
) -> (Trajectory, Option<(usize, Unsupported)>) {
    let n = rotations.len();
    let mut inputs: Vec<InputRow> = rec.rows[..a].iter().map(|r| r.input.clone()).collect();
    for (j, u) in rotations.iter().enumerate() {
        let src = (b + j + 1).saturating_sub(n).max(a + 1).min(b);
        inputs.push(InputRow {
            yaw: u.yaw as f64,
            pitch: u.pitch as f64,
            keys: rec.rows[src - 1].input.keys.clone(),
        });
    }
    inputs.extend(rec.rows[b..].iter().map(|r| r.input.clone()));
    let mut p = match rec.initial.to_player() {
        Ok(p) => p,
        Err(e) => panic!("recorded initial state: {e}"),
    };
    let mut rows = Vec::with_capacity(inputs.len());
    let mut err = None;
    for (i, input) in inputs.into_iter().enumerate() {
        let ti = TickInput {
            y_rot: input.yaw as f32,
            x_rot: input.pitch as f32,
            keys: keys_of(&input.keys).expect("recorded keys parse"),
        };
        if let Err(u) = tick(&mut p, &ti, world) {
            err = Some((i + 1, u));
            break;
        }
        rows.push(Row {
            t: i + 1,
            input,
            state: FullState::from(&p),
        });
    }
    (
        Trajectory {
            header,
            initial: rec.initial.clone(),
            arrivals: Vec::new(),
            rows,
        },
        err,
    )
}
