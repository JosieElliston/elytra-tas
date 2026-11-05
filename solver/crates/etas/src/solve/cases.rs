//! Booster passes as the cases a branching plan covers (ARCHITECTURE.md, Mechanics 3).
//!
//! [`booster::predict`](crate::booster::predict) says which arrivals a pass can bring
//! under each delay; this says how often each combination comes, so a plan can cover
//! the likely ones and say what it leaves out. The frequencies are the fit's evidence
//! (`fit_course.py`'s booster `evidence`): the first boost's lag after the first inner
//! move, and the share of maybe ticks that boosted. Each is pooled over a course's
//! boosters and taken as independent of everything else, which the data is too small
//! to test (as of the 2026-10-20 alpine fit: 74 first boosts, 124 maybe ticks).

use crate::booster::{Visit, look};
use crate::server::Arrival;

/// How often what the booster model leaves open happens.
#[derive(Debug, Clone, PartialEq)]
pub struct Freq {
    /// `first[k]`: the first boost comes on the `k`-th move after the first inner one.
    pub first: Vec<f64>,
    /// A maybe tick boosts.
    pub maybe: f64,
}

impl Freq {
    /// Pooled over the boosters of a course file (the raw JSON: `etas::booster::Booster`
    /// doesn't carry the evidence).
    pub fn from_course_json(course: &serde_json::Value) -> Result<Freq, String> {
        let bs = course["boosters"].as_array().ok_or("no boosters")?;
        let (mut first, mut yes, mut no) = (Vec::<f64>::new(), 0.0, 0.0);
        for b in bs {
            let ev = &b["evidence"];
            let lag = ev["entry_lag"]
                .as_object()
                .ok_or("booster without entry_lag")?;
            for (k, n) in lag {
                let k: usize = k.parse().map_err(|e| format!("entry_lag {k}: {e}"))?;
                if first.len() <= k {
                    first.resize(k + 1, 0.0);
                }
                first[k] += n.as_f64().unwrap_or(0.0);
            }
            yes += ev["maybe_ticks"]["boosted"].as_f64().unwrap_or(0.0);
            no += ev["maybe_ticks"]["not"].as_f64().unwrap_or(0.0);
        }
        let total: f64 = first.iter().sum();
        if total == 0.0 || yes + no == 0.0 {
            return Err("no booster evidence".into());
        }
        Ok(Freq {
            first: first.iter().map(|n| n / total).collect(),
            maybe: yes / (yes + no),
        })
    }
}

/// One way a pass can come out: the arrivals the client sees.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// In tick order; empty when the pass brings no boost.
    pub arrivals: Vec<Arrival>,
    /// The delays that bring it, each with its probability: the delay's prior times
    /// this outcome's under it.
    pub delays: Vec<(usize, f64)>,
}

impl Outcome {
    pub fn p(&self) -> f64 {
        self.delays.iter().map(|d| d.1).sum()
    }

    /// The delays given this outcome, renormalized: what the rest of the run knows.
    pub fn posterior(&self) -> Vec<(usize, f64)> {
        let p = self.p();
        self.delays.iter().map(|&(d, q)| (d, q / p)).collect()
    }

    pub fn last_tick(&self) -> Option<usize> {
        self.arrivals.last().map(|a| a.tick)
    }
}

/// The outcomes of one pass, given the pass under each delay (`by_delay`: the same
/// visit of each `booster::Case`) and the delays' probabilities so far. Outcomes with
/// the same arrivals are merged. Their probabilities sum to the prior's. Err when the
/// pass has a move on the ground or none in the inner box, where the model has no
/// timing.
pub fn outcomes(
    by_delay: &[(usize, &Visit)],
    prior: &[(usize, f64)],
    freq: &Freq,
) -> Result<Vec<Outcome>, String> {
    let mut out: Vec<Outcome> = Vec::new();
    let mut add = |arrivals: Vec<Arrival>, d: usize, p: f64| {
        if p == 0.0 {
            return;
        }
        match out.iter_mut().find(|o| o.arrivals == arrivals) {
            Some(o) => o.delays.push((d, p)),
            None => out.push(Outcome {
                arrivals,
                delays: vec![(d, p)],
            }),
        }
    };
    for &(d, pd) in prior {
        let Some(&(_, v)) = by_delay.iter().find(|(dd, _)| *dd == d) else {
            return Err(format!("no visit for delay {d}"));
        };
        if v.first_by.is_none() {
            return Err("a pass without timing (on the ground, or no inner move)".into());
        }
        let e = v
            .inner
            .iter()
            .position(|&i| i)
            .expect("first_by implies an inner move");
        let mut none = 0.0;
        for (k, &pk) in freq.first.iter().enumerate() {
            let f = e + k;
            let Some((sure, maybe)) = v.given_first(f) else {
                none += pk;
                continue;
            };
            let m = maybe.len();
            for mask in 0u32..(1 << m) {
                let mut a = sure.clone();
                a.extend((0..m).filter(|i| mask >> i & 1 == 1).map(|i| maybe[i]));
                a.sort_by_key(|x| x.tick);
                let ones = mask.count_ones() as i32;
                add(
                    a,
                    d,
                    pd * pk * freq.maybe.powi(ones) * (1.0 - freq.maybe).powi(m as i32 - ones),
                );
            }
        }
        add(Vec::new(), d, pd * none);
    }
    Ok(out)
}

/// The booster model of one pass, a move at a time, for a pass whose later moves
/// depend on its earlier arrivals (a boost that lands while it is still inside moves
/// where the rest of it goes, so `booster::predict` on the unboosted moves would give
/// the wrong candidates). The same rules as `booster::Visit::given_first` under one
/// delay: the first boost on the `k`-th move after the first inner one with
/// probability `first[k]` (as a hazard: given none before), then `settle - 1` maybe
/// moves, then every inner move sure, the rest maybe; moves before the first inner one
/// never boost (the data has no boost there).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pass {
    /// Moves inside so far.
    pub moves: usize,
    /// Visit index of the first inner move, and of the first boost.
    pub inner_at: Option<usize>,
    pub first_at: Option<usize>,
    /// A move left the region after the pass began.
    pub ended: bool,
}

impl Pass {
    pub const START: Pass = Pass {
        moves: 0,
        inner_at: None,
        first_at: None,
        ended: false,
    };

    /// The move just sent: inside the working box, the inner box. Returns the ways it
    /// can go: (boosts, probability, the pass after). Err for a move on the ground
    /// inside (no timing there) or one that comes back after the pass ended.
    pub fn step(
        &self,
        inside: bool,
        inner: bool,
        on_ground: bool,
        settle: usize,
        freq: &Freq,
    ) -> Result<Vec<(bool, f64, Pass)>, String> {
        if !inside {
            let ended = self.ended || self.moves > 0;
            return Ok(vec![(false, 1.0, Pass { ended, ..*self })]);
        }
        if self.ended {
            return Err("the pass comes back into the region".into());
        }
        if on_ground {
            return Err("a move on the ground inside the region".into());
        }
        let i = self.moves;
        let mut next = Pass {
            moves: i + 1,
            ..*self
        };
        if next.inner_at.is_none() && inner {
            next.inner_at = Some(i);
        }
        let p_boost = match (next.first_at, next.inner_at) {
            (Some(f), _) if i - f < settle || !inner => freq.maybe,
            (Some(_), _) => 1.0,
            (None, None) => 0.0,
            (None, Some(e)) => {
                let k = i - e;
                let rest: f64 = freq.first.iter().skip(k).sum();
                if k >= freq.first.len() || rest <= 0.0 {
                    0.0
                } else {
                    (freq.first[k] / rest).min(1.0)
                }
            }
        };
        let mut out = Vec::new();
        if p_boost > 0.0 {
            let mut b = next;
            b.first_at = b.first_at.or(Some(i));
            out.push((true, p_boost, b));
        }
        if p_boost < 1.0 {
            out.push((false, 1.0 - p_boost, next));
        }
        Ok(out)
    }
}

/// The first tick before which outcome `a` and `b` observe different things (an
/// arrival at a tick in one but not the other, or different events there); None if
/// the same.
pub fn parting(a: &Outcome, b: &Outcome) -> Option<usize> {
    let ticks = a.arrivals.iter().chain(&b.arrivals).map(|x| x.tick);
    let mut ts: Vec<usize> = ticks.collect();
    ts.sort();
    ts.into_iter().find(|&t| {
        let at = |o: &Outcome| o.arrivals.iter().find(|x| x.tick == t).map(|x| x.event);
        at(a) != at(b)
    })
}

/// The first line outcome `i` of `outs` flies on its own: every other outcome has
/// parted from it by then, and its last arrival is in (handled before that line).
/// Lines before it are shared with some other outcome, so their rotations can't
/// depend on `i`. None for an outcome without arrivals.
pub fn alone_from(outs: &[Outcome], i: usize) -> Option<usize> {
    let last = outs[i].last_tick()?;
    let part = (0..outs.len())
        .filter(|&j| j != i)
        .filter_map(|j| parting(&outs[i], &outs[j]))
        .max()
        .unwrap_or(0);
    Some(last.max(part))
}

/// Whether a booster's velocity for this rotation is safe from the last place of the
/// trig: no axis of `speed × look` sits within `eps` (relative, per 1/8000 step) of a
/// step of the velocity packet's truncation. why? `booster::payload` matches the
/// server's `Math.sin`/`cos` only to the last place, and at a step that place picks
/// the step, so the plan's payload would be a step off the one that arrives.
pub fn payload_is_stable(speed: f64, y_rot: f32, x_rot: f32, eps: f64) -> bool {
    look(y_rot, x_rot).iter().all(|&c| {
        let v = speed * c;
        if v.abs() >= 3.9 {
            return true;
        }
        let q = v * 8000.0;
        (q - q.round()).abs() > eps
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::booster::{Booster, SentMove, payload, predict};
    use crate::server::ServerEvent;

    /// A 3-move pass under delays 3 and 4, payloads told apart by a yaw step: every
    /// outcome's arrivals are distinct, the probabilities sum to 1, and the delay is
    /// known from any outcome with a boost.
    #[test]
    fn three_move_pass() {
        let b = Booster {
            name: "b".into(),
            speed: 1.4,
            region: serde_json::from_value(serde_json::json!({
                "working": {"min": [0.0, 0.0, 0.0], "max": [10.0, 10.0, 2.5]},
                "inner": {"min": [0.0, 0.0, 0.0], "max": [10.0, 10.0, 2.5]},
                "outer": {"min": [0.0, 0.0, 0.0], "max": [10.0, 10.0, 2.5]},
                "faces": {}, "status": "test"
            }))
            .unwrap(),
            delays: vec![3, 4],
            entry: 3,
            settle: 3,
            status: "test".into(),
            note: String::new(),
        };
        let moves: Vec<SentMove> = (0..5)
            .map(|i| SentMove {
                tick: 10 + i,
                pos: [5.0, 5.0, -1.0 + i as f64],
                y_rot: 0.05 * i as f32,
                x_rot: 10.0,
                on_ground: false,
            })
            .collect();
        let cases = predict(&b, &moves);
        let by: Vec<(usize, &Visit)> = cases.iter().map(|c| (c.delay, &c.visits[0])).collect();
        assert_eq!(by[0].1.candidates.len(), 3);
        let freq = Freq {
            first: vec![0.7, 0.2, 0.05, 0.05],
            maybe: 0.6,
        };
        let outs = outcomes(&by, &[(3, 0.5), (4, 0.5)], &freq).unwrap();
        let total: f64 = outs.iter().map(|o| o.p()).sum();
        assert!((total - 1.0).abs() < 1e-12, "{total}");
        for o in &outs {
            assert!(o.arrivals.is_empty() || o.delays.len() == 1, "{o:?}");
            if !o.arrivals.is_empty() {
                assert!(matches!(
                    o.arrivals[0].event,
                    ServerEvent::SetVelocity { .. }
                ));
            }
        }
        // no boost: f = 3 is past the 3 moves
        let none = outs.iter().find(|o| o.arrivals.is_empty()).unwrap();
        assert!((none.p() - 0.05).abs() < 1e-12);
        for i in 0..outs.len() {
            if let Some(t) = alone_from(&outs, i) {
                assert!(t >= outs[i].last_tick().unwrap());
            }
        }
        // the move-at-a-time model gives the same outcomes on the same moves
        for (d, v) in &by {
            let mut leaves: Vec<(Vec<Arrival>, f64)> = Vec::new();
            #[allow(clippy::too_many_arguments)]
            fn walk(
                moves: &[SentMove],
                b: &Booster,
                d: usize,
                freq: &Freq,
                k: usize,
                pass: Pass,
                acc: (Vec<Arrival>, f64),
                out: &mut Vec<(Vec<Arrival>, f64)>,
            ) {
                let Some(m) = moves.get(k) else {
                    out.push(acc);
                    return;
                };
                let (inside, inner) = (
                    b.region.working.contains(m.pos),
                    b.region.inner.contains(m.pos),
                );
                for (boost, p, next) in pass.step(inside, inner, false, b.settle, freq).unwrap() {
                    let mut a = acc.clone();
                    a.1 *= p;
                    if boost {
                        a.0.push(Arrival {
                            tick: m.tick + d,
                            event: ServerEvent::SetVelocity {
                                v: payload(b.speed, m.y_rot, m.x_rot),
                            },
                        });
                    }
                    walk(moves, b, d, freq, k + 1, next, a, out);
                }
            }
            walk(
                &moves,
                &b,
                *d,
                &freq,
                0,
                Pass::START,
                (vec![], 0.5),
                &mut leaves,
            );
            assert_eq!(v.candidates.len(), 3);
            for o in &outs {
                let Some(&(_, want)) = o.delays.iter().find(|x| x.0 == *d) else {
                    continue;
                };
                let q: f64 = leaves
                    .iter()
                    .filter(|l| l.0 == o.arrivals)
                    .map(|l| l.1)
                    .sum();
                assert!((q - want).abs() < 1e-12, "{o:?}: {q}");
            }
        }
    }
}
