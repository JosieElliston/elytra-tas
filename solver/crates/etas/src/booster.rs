//! Boosters: EMC volumes that set your velocity to `speed` times your look. Measured by
//! `boosters.py`; a course's boosters come from `fit_course.py`, whose docstring is the
//! format. In tick terms, for a run of consecutive ticks whose moves are inside the
//! region (a [`Visit`]):
//!
//! - the move of tick n, if it triggers, puts [`payload`] of its rotation on the
//!   physics of tick n + delay, one delay per [`Case`];
//! - the first boost comes within `entry` ticks of the first move inside the region's
//!   inner box; the `settle - 1` ticks after it may or may not boost (the data doesn't
//!   say which: it isn't a function of the moves); every later tick inside the inner
//!   box does; no tick outside the region does.
//!
//! Visits with a move on the ground follow none of the timing rules (standing in a
//! booster boosted every 10-25 ticks), so their ticks are all "maybe".
//!
//! Out of the model (tests/booster.rs counts them; 16 of 265 recorded alpine boosts):
//! a delay that changes within a visit (moves near a server tick edge), a delay
//! outside `delays` (server lag), and two boosts on one tick (two server ticks'
//! packets handled before one client tick; the later wins). Their payloads still are
//! the candidates', a tick off.

use serde::Deserialize;

use crate::course::Region;
use crate::server::{Arrival, ServerEvent};

#[derive(Debug, Clone, Deserialize)]
pub struct Booster {
    pub name: String,
    /// Blocks per tick.
    pub speed: f64,
    /// Over the position of the move whose rotation a boost carries.
    pub region: Region,
    /// Ticks from a triggering move's tick to the tick whose physics the velocity acts on.
    pub delays: Vec<usize>,
    pub entry: usize,
    pub settle: usize,
    pub status: String,
    #[serde(default)]
    pub note: String,
}

/// What the server holds after a client tick's move packet: the tick's end position
/// and its rotation (the last one sent, for a packet without one).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SentMove {
    pub tick: usize,
    pub pos: [f64; 3],
    pub y_rot: f32,
    pub x_rot: f32,
    pub on_ground: bool,
}

/// `Mth.wrapDegrees(float)`: the server's copy of a received yaw.
pub fn wrap_degrees(d: f32) -> f32 {
    let mut r = d % 360.0;
    if r >= 180.0 {
        r -= 360.0;
    }
    if r < -180.0 {
        r += 360.0;
    }
    r
}

/// Bukkit's `Location.getDirection`: double trig of the float rotation (yaw as the
/// server holds it, wrapped).
pub fn look(y_rot: f32, x_rot: f32) -> [f64; 3] {
    let y = (wrap_degrees(y_rot) as f64).to_radians();
    let p = (x_rot as f64).to_radians();
    let xz = p.cos();
    [-xz * y.sin(), -p.sin(), xz * y.cos()]
}

/// The pre-1.21.9 velocity packet: `(int)(clamp(v, -3.9, 3.9) * 8000)` per axis, read
/// back as `/ 8000.0`. why this old packet? EMC's server speaks it, and ViaVersion
/// re-encodes the result for 26.2 clients.
pub fn short8000(v: [f64; 3]) -> [f64; 3] {
    v.map(|c| (c.clamp(-3.9, 3.9) * 8000.0) as i32 as f64 / 8000.0)
}

/// `net.minecraft.network.LpVec3` write then read: what the 26.2 client decodes.
pub fn lpvec3(v: [f64; 3]) -> [f64; 3] {
    let v = v.map(|c| {
        if c.is_nan() {
            0.0
        } else {
            c.clamp(-1.7179869183E10, 1.7179869183E10)
        }
    });
    let m = v[0].abs().max(v[1].abs().max(v[2].abs()));
    if m < 3.051944088384301E-5 {
        return [0.0; 3];
    }
    let scale = m.ceil();
    v.map(|c| {
        // Math.round of a value >= 0, which f64::round matches
        let q = ((c / scale * 0.5 + 0.5) * 32766.0).round() as i64 & 32767;
        ((q as f64).min(32766.0) * 2.0 / 32766.0 - 1.0) * scale
    })
}

/// The velocity the client receives from a booster of `speed` for a move with this
/// rotation, bit for bit (up to `Math.sin` against Rust's `sin` in the last place,
/// which the 1/8000 truncation hides except at a boundary).
pub fn payload(speed: f64, y_rot: f32, x_rot: f32) -> [f64; 3] {
    lpvec3(short8000(look(y_rot, x_rot).map(|c| speed * c)))
}

/// One pass through a booster under one delay case.
#[derive(Debug, Clone, PartialEq)]
pub struct Visit {
    /// One per inside tick, in tick order: the boost that tick's move would trigger.
    pub candidates: Vec<Arrival>,
    /// Per candidate: whether its move is inside the region's inner box.
    pub inner: Vec<bool>,
    /// The last candidate index the first boost can come on; None if no move is inside
    /// the inner box or the visit has a move on the ground (then nothing is sure).
    pub first_by: Option<usize>,
    pub settle: usize,
}

/// What a recorded visit did against the model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Checked {
    pub matched: usize,
    /// Sure candidates that didn't arrive.
    pub sure_misses: Vec<Arrival>,
    /// No boost at all, though the visit reached `first_by`.
    pub no_first: bool,
}

impl Visit {
    /// The arrivals if the first boost comes on candidate `f`, as (sure, maybe); None if
    /// the model rules `f` out.
    pub fn given_first(&self, f: usize) -> Option<(Vec<Arrival>, Vec<Arrival>)> {
        if f >= self.candidates.len() || self.first_by.is_some_and(|b| f > b) {
            return None;
        }
        let (mut sure, mut maybe) = (vec![self.candidates[f]], vec![]);
        for i in f + 1..self.candidates.len() {
            if self.first_by.is_some() && i >= f + self.settle && self.inner[i] {
                sure.push(self.candidates[i]);
            } else {
                maybe.push(self.candidates[i]);
            }
        }
        Some((sure, maybe))
    }

    /// The span of arrival ticks this visit can put boosts on.
    pub fn span(&self) -> std::ops::RangeInclusive<usize> {
        self.candidates[0].tick..=self.candidates[self.candidates.len() - 1].tick
    }

    /// Check recorded booster arrivals (those in `span`, sorted by tick) against the
    /// visit. Err when one isn't a candidate or the first comes after `first_by`.
    pub fn check(&self, recorded: &[Arrival]) -> Result<Checked, String> {
        let mut hit = vec![false; self.candidates.len()];
        for a in recorded {
            let i = self
                .candidates
                .iter()
                .position(|c| c == a)
                .ok_or_else(|| format!("arrival {a:?} is no candidate of the visit"))?;
            hit[i] = true;
        }
        let Some(f) = hit.iter().position(|&h| h) else {
            let no_first = self.first_by.is_some_and(|b| b < self.candidates.len());
            return Ok(Checked {
                no_first,
                ..Checked::default()
            });
        };
        if self.first_by.is_some_and(|b| f > b) {
            return Err(format!(
                "first boost on candidate {f}, the model has it by {:?}",
                self.first_by
            ));
        }
        let (sure, _) = self.given_first(f).expect("f is allowed");
        let sure_misses = sure.into_iter().filter(|s| !recorded.contains(s)).collect();
        Ok(Checked {
            matched: recorded.len(),
            sure_misses,
            no_first: false,
        })
    }
}

/// The visits of one delay case.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub delay: usize,
    pub visits: Vec<Visit>,
}

impl Case {
    /// Every arrival this case allows, sure or not, in tick order.
    pub fn possible(&self) -> Vec<Arrival> {
        self.visits
            .iter()
            .flat_map(|v| v.candidates.iter().copied())
            .collect()
    }
}

/// The booster's arrivals for a move history (sorted by tick; one move per tick), one
/// case per delay. A visit is a run of consecutive ticks whose moves are inside the
/// working region; a tick without a move ends it.
pub fn predict(b: &Booster, moves: &[SentMove]) -> Vec<Case> {
    let (working, inner) = (&b.region.working, &b.region.inner);
    let mut runs: Vec<Vec<&SentMove>> = vec![];
    for m in moves.iter().filter(|m| working.contains(m.pos)) {
        match runs.last_mut() {
            Some(r) if r.last().is_some_and(|l| l.tick + 1 == m.tick) => r.push(m),
            _ => runs.push(vec![m]),
        }
    }
    b.delays
        .iter()
        .map(|&delay| Case {
            delay,
            visits: runs
                .iter()
                .map(|r| {
                    let inner: Vec<bool> = r.iter().map(|m| inner.contains(m.pos)).collect();
                    let grounded = r.iter().any(|m| m.on_ground);
                    Visit {
                        candidates: r
                            .iter()
                            .map(|m| Arrival {
                                tick: m.tick + delay,
                                event: ServerEvent::SetVelocity {
                                    v: payload(b.speed, m.y_rot, m.x_rot),
                                },
                            })
                            .collect(),
                        first_by: (!grounded)
                            .then(|| inner.iter().position(|&i| i).map(|e| e + b.entry))
                            .flatten(),
                        inner,
                        settle: b.settle,
                    }
                })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A float as the logger writes it: widened to double.
    fn f(x: f64) -> f32 {
        x as f32
    }

    /// Velocities as the logger recorded them (`set_entity_motion` of self), for the
    /// rotation of the move that triggered them.
    #[test]
    fn payload_matches_recorded() {
        // alpine ring, 2026-10-20 01:58 log, client tick 1811 (held rotation)
        let v = payload(1.4, f(92.48558044433594), f(9.151168823242188));
        assert_eq!(
            v,
            [
                -1.380821583348593,
                -0.22266984068851858,
                -0.05981810413233224
            ]
        );
        // the same log, tick 270: the gold ring at 2.5 b/t
        let v = payload(2.5, f(-22.5859432220459), f(-29.790061950683594));
        assert_eq!(
            v,
            [0.8331807361289139, 1.2420802050906428, 2.003112982970152]
        );
        // classic urban slop, 2026-11-01 15:46 log, tick 3138: 5 b/t, z clamped to 3.9
        // (LpVec3 then puts it on its grid), an unwrapped yaw
        let v = payload(5.0, f(-362.0041809082031), f(16.55002212524414));
        assert_eq!(
            v,
            [0.16749069157053054, -1.42415918940365, 3.8998962339009946]
        );
    }

    #[test]
    fn wrap_is_javas() {
        assert_eq!(wrap_degrees(f(-182.2457733154297)), f(177.7542266845703));
        assert_eq!(wrap_degrees(180.0), -180.0);
        assert_eq!(wrap_degrees(-540.5), 179.5);
    }

    #[test]
    fn clamp_binds_per_axis() {
        // a 5 b/t booster looking straight along +z: z clamps to 3.9, which LpVec3 (scale
        // 4, a grid of 8/32766) moves by at most half a step
        let v = payload(5.0, 0.0, 0.0);
        assert_eq!(v[0], 0.0);
        assert!((v[2] - 3.9).abs() <= 4.0 / 32766.0, "{v:?}");
    }
}
