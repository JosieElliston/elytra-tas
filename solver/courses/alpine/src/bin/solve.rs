//! Phase `solve`: alpine as a branching plan over its three boosters.
//!
//! The route is the recorded run's (`recorded.tas`, run 2 of the 2026-10-20 00:09
//! replay): ring_1, gold_ring, ring_3, the finish. A booster replaces the velocity, so
//! the run splits at each pass into stretches that need only reach a point:
//!
//! 1. The recorded prefix to tick [`T0`] (the deploy and the start-area bounce,
//!    before the timer starts), then per stretch objective `min_ticks_to_point`: from
//!    the state the stretch starts in, reach a point just before the next booster's
//!    region (the recorded one along the ring's normal, centered across it), velocity
//!    free, in as few ticks as the faithful kernel flies clean (`etas::solve::reach`),
//!    seeded with the recorded controls, or the pilot's (below) where those fail.
//! 2. The pass: the recording's rotation on its last move inside is held from there
//!    until the plan knows the outcome, its yaw stepped by [`DITHER`] per line so that
//!    every move's payload differs: an arrival then says which move and which delay it
//!    came from, and the delay (a session's, per the data) is known from the first
//!    booster on. The stretch's end is moved across the ring until the pass leaves
//!    through the inner box's center, and if some outcome still touches the frame, to
//!    the point of a grid that loses least to contact.
//! 3. The outcomes, enumerated a move at a time (`solve::cases::Pass`, the booster
//!    model with the fit's frequencies) on the pass as each one flies it, since a
//!    boost that lands while the pass is still inside moves where the rest goes. Each
//!    one the plan covers (an arrival, probability given the path so far at least
//!    [`COVER_MIN`]) flies on from the first line no other covered outcome shares,
//!    solving its own next stretch; the others, and those that touch, abort in
//!    playback (the run is retried). The pilot, run first, covers only the likeliest.
//! 4. The last stretch reaches a point just inside the finish's inner box, then
//!    [`TAIL`] ticks for "Course complete!" to arrive.
//!
//! The paths are merged into one plan (`plan::branched`), written to
//! `runs/solve.plan.tas`, and every path verified (`plan::verify_paths`). Each path's
//! arrivals are also checked against `booster::predict` on that path's own moves (the
//! audit; a path that fails it is dropped as uncovered). It prints each path's timer,
//! the probability that an attempt stays covered, and the expected time per
//! completed run.
//!
//! `cargo run --release -p alpine --bin solve [--cover P] [--scan]`: `--cover` sets
//! [`COVER_MIN`]; `--scan` writes no plan, and instead flies the pilot (the likeliest
//! outcome of every pass) with each ring's held rotation moved by a few degrees, to
//! see what the choice of held rotation is worth.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Instant;

use etas::booster::{Booster, SentMove, payload, predict};
use etas::course::Course;
use etas::plan::{self, Action, Event, PathTape, React};
use etas::server::{self, Arrival, ServerEvent};
use etas::solve::cases::{Freq, Outcome, Pass, alone_from, payload_is_stable};
use etas::solve::ddp::{U, X, control, lead_of};
use etas::solve::reach::to_point;
use etas::state::{FullState, keys_of};
use etas::tape::{Tape, TapeRow};
use etas::trajectory::{Trajectory, from_tape};
use etas::verify;
use port::{LocalPlayer, TickInput, tick};
use rayon::prelude::*;
use world::World;

const OBJECTIVE: &str = "min_ticks_to_point";
/// The recorded prefix: through the start-area bounce (landing 20, jump 21), before
/// the timer starts (judged at 42 on the recording).
const T0: usize = 38;
/// Degrees of yaw between consecutive inside moves of a pass, so their payloads
/// differ (a 1.4 b/t booster moves ~0.024 b/t per degree, against a 1/8000 step).
const DITHER: f32 = 0.05;
/// An outcome is covered when its probability, given the path so far, is at least this.
const COVER_MIN: f64 = 0.02;
/// The delay prior. why even? of the four alpine sessions in the booster fixture, two
/// boosted mostly at 3 and two at 4 (`tests/data/boosters/alpine.json`).
const DELAY_PRIOR: [(usize, f64); 2] = [(3, 0.5), (4, 0.5)];
/// Ticks flown after the finish tick: EMC's chat comes 2 to 5 later
/// (`observation.tick_chat_lag`, widened).
const TAIL: usize = 10;
/// Tail pitch, degrees: up, away from the ground past the finish.
const TAIL_PITCH: f32 = -20.0;
/// A stretch's repaired end may be this far from its point.
const REACH_TOL: f64 = 0.05;
/// Rounds of moving a stretch's end across a ring to center the pass, and the
/// offset (blocks, per axis) that is centered enough.
const CENTER_ROUNDS: usize = 4;
const CENTER_TOL: f64 = 0.3;
/// The grid of stretch ends tried across a ring when the centered pass touches:
/// `2 GRID + 1` steps of `GRID_STEP` blocks per axis.
const GRID: i32 = 3;
const GRID_STEP: f64 = 0.75;
/// Ticks after a lost outcome's pass starts that playback aborts (its arrivals come
/// 3 to 8 ticks in), and per attempt the ticks besides the plan's lines: reset,
/// `settle=10`, 20 after "Course complete!" (README, Playback). Both rough, for the
/// expected time per completed run.
const ABORT_AFTER: usize = 6;
const ATTEMPT_OVERHEAD: usize = 35;
/// Held lines a pass may take, from the point before the region to its last arrival.
const PASS_LINES: usize = 14;

/// A booster on the route, as the recording flew it.
struct Ring {
    booster: Booster,
    /// Recorded: the last tick before its first move in the region, the rotation of
    /// its last move inside, and its last arrival.
    pre: usize,
    rot: (f32, f32),
    resume: usize,
    /// Where the stretch into it ends, first guess: the recorded point at `pre` along
    /// the inner box's thinnest axis (`normal`, the ring's), the box's `center` across
    /// it. why not the recorded point? the recorded passes run near the frame (both
    /// 00:09 runs hit the gold ring's side), and a held rotation then flies into it.
    target: [f64; 3],
    normal: usize,
    center: [f64; 3],
}

/// A path being built: the player after its last row.
#[derive(Clone)]
struct Node {
    p: LocalPlayer,
    rows: Vec<TapeRow>,
    arrivals: Vec<Arrival>,
    /// Probability of the outcomes so far, and the delay given them.
    prob: f64,
    delays: Vec<(usize, f64)>,
    labels: Vec<String>,
}

/// What a path came to, or what it gave up on (an uncovered or unsolved outcome).
enum Leaf {
    Done(Node),
    /// `at`: the tick it was given up at; playback aborts a few ticks later, where
    /// the arrival nothing expects comes.
    Lost {
        prob: f64,
        at: usize,
        why: String,
    },
}

struct Ctx<'a> {
    course: &'a Course,
    world: &'a World,
    rec: &'a Trajectory,
    rings: Vec<Ring>,
    freq: Freq,
    finish: [f64; 3],
    /// The recording's finish tick (verify's), where its last stretch ends.
    rec_finish: usize,
    /// The pilot covers only the likeliest outcome of each pass, and leaves its
    /// stretches' controls in `fallback` for seeds where the recorded ones fail.
    pilot: bool,
    fallback: Mutex<Vec<Option<Vec<U>>>>,
    cover: f64,
    /// Solve every stretch from both seeds and keep the shorter.
    both_seeds: bool,
    /// Per ring, degrees added to the recorded held rotation (yaw, pitch).
    rot_offset: Vec<(f32, f32)>,
    log: Mutex<Vec<String>>,
}

fn flight(s: &FullState) -> X {
    let (p, v) = (s.position, s.delta_movement);
    [p[0], p[1], p[2], v[0], v[1], v[2]]
}

impl Node {
    fn t(&self) -> usize {
        self.rows.len()
    }

    /// Fly one tick (its arrivals first) and record the line.
    fn fly(&mut self, yaw: f32, pitch: f32, keys: &str, world: &World) -> Result<(), String> {
        let t = self.t() + 1;
        server::apply_at(&mut self.p, &self.arrivals, t, world);
        let ti = TickInput {
            y_rot: yaw,
            x_rot: pitch,
            keys: keys_of(keys)?,
        };
        tick(&mut self.p, &ti, world).map_err(|u| format!("tick {t}: {u:?}"))?;
        self.rows.push(TapeRow {
            t,
            yaw,
            pitch,
            keys: keys.to_string(),
            pos: [self.p.position.x, self.p.position.y, self.p.position.z],
            ground: self.p.on_ground,
        });
        Ok(())
    }

    /// The flight state the next line starts from: its arrivals applied.
    fn x_next(&self, world: &World) -> Result<X, String> {
        let mut p = self.p.clone();
        server::apply_at(&mut p, &self.arrivals, self.t() + 1, world);
        let s = FullState::from(&p);
        if !(s.fall_flying && !s.on_ground && s.pose == "fall_flying") {
            return Err(format!("tick {}: not in normal flight", self.t()));
        }
        Ok(flight(&s))
    }
}

impl Ctx<'_> {
    fn note(&self, s: String) {
        self.log.lock().unwrap().push(s);
    }

    fn rec_state(&self, t: usize) -> &FullState {
        if t == 0 {
            &self.rec.initial
        } else {
            &self.rec.rows[t - 1].state
        }
    }

    /// The recorded controls of ticks `a..=b` (leads off the recorded velocity), and
    /// the recorded positions there.
    fn seed(&self, a: usize, b: usize) -> (Vec<U>, Vec<[f64; 3]>) {
        let u = (a..=b)
            .map(|t| {
                let i = &self.rec.rows[t - 1].input;
                control(lead_of(&flight(self.rec_state(t - 1)), i.yaw), i.pitch)
            })
            .collect();
        let path = (a - 1..=b).map(|t| self.rec_state(t).position).collect();
        (u, path)
    }

    /// Fly from `node` to `target` in least ticks; the recorded ticks `a..=b` seed it.
    /// Stretch `k` (into ring `k`, or the finish) from `node` to `target` in least
    /// ticks: seeded with the recorded ticks `a..=b`, and failing that with the
    /// pilot's solution of the same stretch.
    fn stretch(
        &self,
        node: &mut Node,
        k: usize,
        target: [f64; 3],
        a: usize,
        b: usize,
    ) -> Result<(), String> {
        let x0 = node.x_next(self.world)?;
        let (seed, path) = self.seed(a, b);
        let accept = |rep: &etas::solve::refly::Repaired| {
            let e = rep.flown.last().unwrap();
            (0..3).map(|k| (e[k] - target[k]).abs()).fold(0.0, f64::max) <= REACH_TOL
        };
        let yaw_from = node.p.y_rot as f64;
        let mut r = to_point(self.world, x0, yaw_from, target, &seed, &path, accept);
        let pilot_seed = self.fallback.lock().unwrap()[k].clone();
        if let Some(u) = pilot_seed
            && (self.both_seeds || r.is_err())
        {
            let q = to_point(self.world, x0, yaw_from, target, &u, &path, accept);
            r = match (r, q) {
                (Ok(a), Ok(b)) => Ok(if b.inputs.len() < a.inputs.len() { b } else { a }),
                (Ok(a), Err(_)) => Ok(a),
                (Err(_), Ok(b)) => Ok(b),
                (Err(a), Err(b)) => Err(format!("{a} (and from the pilot's: {b})")),
            };
        }
        let r = r?;
        if self.pilot {
            let mut prev = x0;
            let u: Vec<U> = r
                .inputs
                .iter()
                .zip(&r.flown[1..])
                .map(|(i, x)| {
                    let c = control(lead_of(&prev, i.yaw as f64), i.pitch as f64);
                    prev = *x;
                    c
                })
                .collect();
            self.fallback.lock().unwrap()[k] = Some(u);
        }
        let t = node.t() + 1;
        for u in &r.inputs {
            node.fly(u.yaw, u.pitch, "-", self.world)?;
        }
        self.note(format!(
            "[{}] stretch from tick {t}: {} ticks (generic {}, recorded {}), {:.1} s; {}",
            node.labels.join(" "),
            r.inputs.len(),
            r.generic,
            b + 1 - a,
            r.secs,
            r.tried.join(" ")
        ));
        Ok(())
    }

    fn sent(rows: &[TapeRow]) -> Vec<SentMove> {
        rows.iter()
            .map(|r| SentMove {
                tick: r.t,
                pos: r.pos,
                y_rot: r.yaw,
                x_rot: r.pitch,
                on_ground: r.ground,
            })
            .collect()
    }

    /// The held lines of a pass through ring `k` from `node`: the recorded rotation,
    /// yaw stepped by [`DITHER`] per line so every line's payload differs, nudged until
    /// every payload is off the truncation's edges.
    fn schedule(&self, k: usize) -> Result<Vec<(f32, f32)>, String> {
        let ring = &self.rings[k];
        let b = &ring.booster;
        let (mut yaw0, mut pitch) = ring.rot;
        yaw0 += self.rot_offset[k].0;
        pitch += self.rot_offset[k].1;
        for _ in 0..40 {
            let sched: Vec<(f32, f32)> = (0..PASS_LINES)
                .map(|i| (yaw0 + DITHER * i as f32, pitch))
                .collect();
            let pays: Vec<[f64; 3]> = sched.iter().map(|&(y, p)| payload(b.speed, y, p)).collect();
            let distinct = (0..pays.len()).all(|i| (0..i).all(|j| pays[i] != pays[j]));
            let stable = sched
                .iter()
                .all(|&(y, p)| payload_is_stable(b.speed, y, p, 1e-9));
            if distinct && stable {
                return Ok(sched);
            }
            yaw0 += 0.003;
            pitch += 0.002;
        }
        Err(format!(
            "{}: no held rotation with stable, distinct payloads",
            b.name
        ))
    }

    /// Every way the pass from `node` can go, flown: the held lines from `sched`, the
    /// booster model a move at a time (`cases::Pass`) under each delay in `node.delays`,
    /// each boost's arrival `delay` ticks after its move. A leaf is the pass once it
    /// has left the region and every arrival is in. Contact during the pass loses the
    /// branch (the server's wall-hit echo would come as a velocity nothing expects).
    fn outcomes(
        &self,
        node: &Node,
        k: usize,
        sched: &[(f32, f32)],
    ) -> Result<Vec<Outcome>, String> {
        let b = &self.rings[k].booster;
        struct W<'a> {
            ctx: &'a Ctx<'a>,
            b: &'a Booster,
            sched: &'a [(f32, f32)],
            start: usize,
            d: usize,
            out: Vec<(Vec<Arrival>, f64)>,
            err: Option<String>,
        }
        fn walk(w: &mut W, n: Node, pass: Pass, p: f64) {
            if w.err.is_some() || p == 0.0 {
                return;
            }
            let i = n.t() - w.start;
            let pending = n.arrivals.iter().any(|a| a.tick > n.t());
            if pass.ended && !pending {
                let new = n
                    .arrivals
                    .iter()
                    .filter(|a| a.tick > w.start)
                    .copied()
                    .collect();
                w.out.push((new, p));
                return;
            }
            let Some(&(y, pt)) = w.sched.get(i) else {
                w.err = Some(format!(
                    "{}: the pass outlasts its {} held lines",
                    w.b.name,
                    w.sched.len()
                ));
                return;
            };
            let mut n = n;
            if let Err(e) = n.fly(y, pt, "-", w.ctx.world) {
                w.err = Some(format!("{}: {e}", w.b.name));
                return;
            }
            let s = FullState::from(&n.p);
            if s.horizontal_collision || s.vertical_collision {
                // a lost branch: its probability goes nowhere, so coverage counts it lost
                w.ctx.note(format!(
                    "[{}] {}: contact at tick {} {:?}",
                    n.labels.join(" "),
                    w.b.name,
                    n.t(),
                    s.position
                ));
                return;
            }
            let pos = s.position;
            let steps = match pass.step(
                w.b.region.working.contains(pos),
                w.b.region.inner.contains(pos),
                s.on_ground,
                w.b.settle,
                &w.ctx.freq,
            ) {
                Ok(x) => x,
                Err(e) => {
                    w.err = Some(format!("{}: {e}", w.b.name));
                    return;
                }
            };
            for (boost, q, next) in steps {
                let mut c = n.clone();
                if boost {
                    c.arrivals.push(Arrival {
                        tick: c.t() + w.d,
                        event: ServerEvent::SetVelocity {
                            v: payload(w.b.speed, y, pt),
                        },
                    });
                }
                walk(w, c, next, p * q);
            }
        }
        let mut outs: Vec<Outcome> = Vec::new();
        for &(d, pd) in &node.delays {
            let mut w = W {
                ctx: self,
                b,
                sched,
                start: node.t(),
                d,
                out: Vec::new(),
                err: None,
            };
            walk(&mut w, node.clone(), Pass::START, pd);
            if let Some(e) = w.err {
                return Err(e);
            }
            for (a, p) in w.out {
                match outs.iter_mut().find(|o| o.arrivals == a) {
                    Some(o) => o.delays.push((d, p)),
                    None => outs.push(Outcome {
                        arrivals: a,
                        delays: vec![(d, p)],
                    }),
                }
            }
        }
        Ok(outs)
    }

    /// Everything from `node` (at the start of stretch `k`) to the plan's end.
    fn advance(&self, mut node: Node, k: usize) -> Vec<Leaf> {
        let lost = |node: &Node, why: String| {
            vec![Leaf::Lost {
                prob: node.prob,
                at: node.t(),
                why: format!("[{}] {why}", node.labels.join(" ")),
            }]
        };
        // the recorded ticks this stretch replaces: from the last booster's last
        // arrival (or T0) to the tick before the next region (or the finish)
        let a = if k == 0 {
            T0 + 1
        } else {
            self.rings[k - 1].resume
        };
        if k == self.rings.len() {
            if let Err(e) = self.stretch(&mut node, k, self.finish, a, self.rec_finish) {
                return lost(&node, format!("to the finish: {e}"));
            }
            let yaw = node.rows.last().unwrap().yaw;
            for _ in 0..TAIL {
                if let Err(e) = node.fly(yaw, TAIL_PITCH, "-", self.world) {
                    return lost(&node, format!("tail: {e}"));
                }
            }
            return vec![Leaf::Done(node)];
        }
        let ring = &self.rings[k];
        // the stretch, its end moved across the ring until the held pass (without
        // arrivals) leaves through the inner box's center: the approach's drift
        // across the ring carries through the pass, and the frame is closest where
        // it leaves (the L-shaped move clips the frame's corner)
        let sched = match self.schedule(k) {
            Ok(s) => s,
            Err(e) => return lost(&node, e),
        };
        let mut target = ring.target;
        let mut solved = None;
        for round in 0..CENTER_ROUNDS {
            let mut n = node.clone();
            if let Err(e) = self.stretch(&mut n, k, target, a, ring.pre) {
                if solved.is_none() && round + 1 == CENTER_ROUNDS {
                    return lost(&node, format!("to {}: {e}", ring.booster.name));
                }
                continue;
            }
            // where the pass leaves the ring: its last move inside and the next one
            let mut probe = n.clone();
            let (mut inside, mut exit) = (0, None);
            let mut last = None;
            for &(y, p) in &sched {
                if probe.fly(y, p, "-", self.world).is_err() {
                    break;
                }
                let pos = probe.rows.last().unwrap().pos;
                if ring.booster.region.working.contains(pos) {
                    inside += 1;
                    last = Some(pos);
                } else if let Some(l) = last {
                    exit = Some(std::array::from_fn::<f64, 3, _>(|k| (l[k] + pos[k]) / 2.0));
                    break;
                }
            }
            let Some(c) = exit.filter(|_| inside > 0) else {
                solved = solved.or(Some((n, target)));
                continue;
            };
            let off = (0..3)
                .filter(|&k| k != ring.normal)
                .map(|k| (ring.center[k] - c[k]).abs())
                .fold(0.0, f64::max);
            solved = Some((n, target));
            if off < CENTER_TOL {
                break;
            }
            for k in (0..3).filter(|&k| k != ring.normal) {
                target[k] += ring.center[k] - c[k];
            }
        }
        let (centered, target) = solved.expect("a stretch solved");
        // a boost that lands before the pass is clear of the frame can push it into
        // it (gold_ring's sends you along the ring's plane): if some outcomes touch,
        // move the end across the ring on a grid and keep the one that loses least
        // to contact, then the fastest
        let lost_of = |o: &[Outcome]| 1.0 - o.iter().map(|o| o.p()).sum::<f64>();
        let first = self.outcomes(&centered, k, &sched).map(|o| (centered, o));
        let (n, outs) = match first {
            Ok((n, o)) if lost_of(&o) < 1e-6 => (n, o),
            first => {
                let mut cands: Vec<[f64; 3]> = Vec::new();
                let cross: Vec<usize> = (0..3).filter(|&k| k != ring.normal).collect();
                for i in -GRID..=GRID {
                    for j in -GRID..=GRID {
                        let mut t = target;
                        t[cross[0]] += i as f64 * GRID_STEP;
                        t[cross[1]] += j as f64 * GRID_STEP;
                        cands.push(t);
                    }
                }
                let tried: Vec<(f64, usize, Node, Vec<Outcome>)> = cands
                    .par_iter()
                    .filter_map(|&t| {
                        let mut n = node.clone();
                        self.stretch(&mut n, k, t, a, ring.pre).ok()?;
                        let o = self.outcomes(&n, k, &sched).ok()?;
                        Some(((lost_of(&o) * 1e3).round() / 1e3, n.t(), n, o))
                    })
                    .chain(
                        first
                            .ok()
                            .map(|(n, o)| ((lost_of(&o) * 1e3).round() / 1e3, n.t(), n, o)),
                    )
                    .collect();
                let Some(best) = tried
                    .into_iter()
                    .min_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)))
                else {
                    return lost(&node, format!("{}: no pass on the grid", ring.booster.name));
                };
                self.note(format!(
                    "[{}] {}: grid pick loses {:.3} to contact, stretch ends at tick {}",
                    node.labels.join(" "),
                    ring.booster.name,
                    best.0,
                    best.1
                ));
                (best.2, best.3)
            }
        };
        node = n;
        let reached: f64 = outs.iter().map(|o| o.p()).sum();
        let likeliest = outs
            .iter()
            .filter(|o| !o.arrivals.is_empty())
            .map(|o| o.p())
            .fold(0.0, f64::max);
        let (cover, skip): (Vec<Outcome>, Vec<Outcome>) = outs.into_iter().partition(|o| {
            !o.arrivals.is_empty()
                && if self.pilot {
                    o.p() == likeliest
                } else {
                    o.p() >= self.cover
                }
        });
        let start = node.t();
        let mut leaves: Vec<Leaf> = skip
            .iter()
            .map(|o| Leaf::Lost {
                prob: node.prob * o.p(),
                at: node.t(),
                why: format!(
                    "[{}] {}: uncovered outcome {}",
                    node.labels.join(" "),
                    ring.booster.name,
                    label(o, start)
                ),
            })
            .collect();
        if reached < 1.0 - 1e-9 {
            leaves.push(Leaf::Lost {
                prob: node.prob * (1.0 - reached),
                at: node.t(),
                why: format!(
                    "[{}] {}: wall contact during the pass",
                    node.labels.join(" "),
                    ring.booster.name
                ),
            });
        }
        let children: Vec<Leaf> = (0..cover.len())
            .into_par_iter()
            .flat_map(|i| {
                let o = &cover[i];
                let from = alone_from(&cover, i).expect("covered outcomes have arrivals");
                let mut c = node.clone();
                c.arrivals.extend(o.arrivals.iter().copied());
                c.prob *= o.p();
                c.delays = o.posterior();
                c.labels
                    .push(format!("{}:{}", ring.booster.name, label(o, start)));
                for &(y, p) in &sched[..from - 1 - start] {
                    if let Err(e) = c.fly(y, p, "-", self.world) {
                        return lost(&c, format!("held pass: {e}"));
                    }
                }
                self.advance(c, k + 1)
            })
            .collect();
        leaves.extend(children);
        leaves
    }
}

/// An outcome as `d<delay>:<arrival ticks after the pass's start>`.
fn label(o: &Outcome, start: usize) -> String {
    let d: Vec<String> = o.delays.iter().map(|d| d.0.to_string()).collect();
    let t: Vec<String> = o
        .arrivals
        .iter()
        .map(|a| (a.tick - start).to_string())
        .collect();
    format!(
        "d{}+{}",
        d.join("/"),
        if t.is_empty() {
            "none".into()
        } else {
            t.join(",")
        }
    )
}

/// The route's boosters from the recording: each one's visit and arrivals.
fn rings(course: &Course, rec: &Tape) -> Result<Vec<Ring>, String> {
    let mut out = Vec::new();
    for b in &course.boosters {
        let inside: Vec<&TapeRow> = rec
            .rows
            .iter()
            .filter(|r| b.region.working.contains(r.pos))
            .collect();
        let (Some(first), Some(last)) = (inside.first(), inside.last()) else {
            return Err(format!("the recording doesn't pass {}", b.name));
        };
        if last.t + 1 - first.t != inside.len() {
            return Err(format!("the recording passes {} more than once", b.name));
        }
        let resume = rec
            .arrivals
            .iter()
            .filter(|a| a.tick > first.t && a.tick <= last.t + 8)
            .map(|a| a.tick)
            .max()
            .ok_or(format!("no recorded arrival at {}", b.name))?;
        let pre = first.t - 1;
        let (lo, hi) = (b.region.inner.min, b.region.inner.max);
        let span = |k: usize| hi[k].unwrap() - lo[k].unwrap();
        let normal = (0..3).min_by(|&i, &j| span(i).total_cmp(&span(j))).unwrap();
        let rec_pre = rec.rows[pre - 1].pos;
        let target = std::array::from_fn(|k| {
            if k == normal {
                rec_pre[k]
            } else {
                (lo[k].unwrap() + hi[k].unwrap()) / 2.0
            }
        });
        out.push(Ring {
            booster: b.clone(),
            pre,
            rot: (last.yaw, last.pitch),
            resume,
            target,
            normal,
            center: std::array::from_fn(|k| (lo[k].unwrap() + hi[k].unwrap()) / 2.0),
        });
    }
    out.sort_by_key(|r| r.pre);
    Ok(out)
}

/// Each path's arrivals against the booster model on its own moves: every visit to a
/// booster region is one the plan modeled, under the path's delay, with no sure
/// arrival missing; every arrival is some visit's.
fn audit(course: &Course, p: &PathTape, delay: usize) -> Result<(), String> {
    let moves = Ctx::sent(&p.rows);
    let mut used = vec![false; p.arrivals.len()];
    for b in &course.boosters {
        let cases = predict(b, &moves);
        let c = cases
            .iter()
            .find(|c| c.delay == delay)
            .ok_or(format!("{}: no case for delay {delay}", b.name))?;
        for v in &c.visits {
            let span = v.span();
            let idx: Vec<usize> = (0..p.arrivals.len())
                .filter(|&i| span.contains(&p.arrivals[i].tick))
                .collect();
            let got: Vec<Arrival> = idx.iter().map(|&i| p.arrivals[i]).collect();
            let ch = v.check(&got).map_err(|e| format!("{}: {e}", b.name))?;
            if got.is_empty() {
                return Err(format!(
                    "{}: a visit at ticks {span:?} the plan has no arrival for",
                    b.name
                ));
            }
            if !ch.sure_misses.is_empty() || ch.matched != got.len() {
                return Err(format!(
                    "{}: sure arrivals missing {:?}",
                    b.name, ch.sure_misses
                ));
            }
            for i in idx {
                used[i] = true;
            }
        }
    }
    if let Some(i) = used.iter().position(|u| !u) {
        return Err(format!("arrival {:?} is no booster visit's", p.arrivals[i]));
    }
    Ok(())
}

fn run() -> Result<bool, String> {
    let t_start = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scan = args.iter().any(|a| a == "--scan");
    let cover = match args.iter().position(|a| a == "--cover") {
        Some(i) => args
            .get(i + 1)
            .and_then(|v| v.parse().ok())
            .ok_or("--cover wants a probability")?,
        None => COVER_MIN,
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (course, world) = Course::load(dir)?;
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("course.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let freq = Freq::from_course_json(&raw)?;
    let tape_path = course.repo_root().join(&course.start.source.path);
    let tape = Tape::read(&tape_path)?;
    let (rec, err) = from_tape(&tape, &world, Some(&course), &course.start.source.path);
    if let Some((t, u)) = err {
        return Err(format!("the recording: port stopped at {t}: {u:?}"));
    }
    let base = verify::verify(&course, &world, verify::Plan::Tape(&tape));
    let base_ticks = base
        .timer_tick()
        .zip(base.finish_tick())
        .map(|(s, f)| f - s)
        .ok_or("the recording has no timer or finish")?;
    let fin = &course.region("finish")?.inner;
    let (lo, hi) = (fin.min.map(|m| m.unwrap()), fin.max.map(|m| m.unwrap()));
    // the inner box's top face over its -z edge: where the recording comes in from
    let finish = [(lo[0] + hi[0]) / 2.0, hi[1] - 0.05, lo[2] + 0.05];
    let ctx = Ctx {
        rings: rings(&course, &tape)?,
        course: &course,
        world: &world,
        rec: &rec,
        freq,
        finish,
        rec_finish: base.finish_tick().unwrap(),
        pilot: true,
        fallback: Mutex::new(Vec::new()),
        cover,
        both_seeds: args.iter().any(|a| a == "--both-seeds"),
        rot_offset: Vec::new(),
        log: Mutex::new(Vec::new()),
    };
    println!(
        "frequencies: first boost {:?}, maybe {:.3}; delay prior {DELAY_PRIOR:?}; cover >= {cover}",
        ctx.freq.first, ctx.freq.maybe
    );
    for r in &ctx.rings {
        println!(
            "{}: recorded pre {} rotation {:?} resume {}; target {:?}",
            r.booster.name, r.pre, r.rot, r.resume, r.target
        );
    }

    // the recorded prefix
    let mut root = Node {
        p: tape.start_player()?,
        rows: Vec::new(),
        arrivals: Vec::new(),
        prob: 1.0,
        delays: DELAY_PRIOR.to_vec(),
        labels: Vec::new(),
    };
    for r in &tape.rows[..T0] {
        root.fly(r.yaw, r.pitch, &r.keys, &world)?;
    }
    let mut ctx = ctx;
    ctx.rot_offset = vec![(0.0, 0.0); ctx.rings.len()];
    if scan {
        return scan_held(ctx, root);
    }
    // the pilot, then everything
    *ctx.fallback.lock().unwrap() = vec![None; ctx.rings.len() + 1];
    let pilot = ctx.advance(root.clone(), 0);
    if !pilot.iter().any(|l| matches!(l, Leaf::Done(_))) {
        return Err("the pilot path didn't reach the finish".into());
    }
    ctx.log.lock().unwrap().clear();
    let ctx = Ctx {
        pilot: false,
        ..ctx
    };
    let leaves = ctx.advance(root, 0);
    for l in ctx.log.lock().unwrap().iter() {
        println!("{l}");
    }
    let mut done = Vec::new();
    let (mut lost_p, mut lost_lines) = (0.0, 0.0);
    for l in leaves {
        match l {
            Leaf::Done(n) => {
                let d = n
                    .delays
                    .iter()
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap()
                    .0;
                let pt = PathTape {
                    rows: n.rows.clone(),
                    arrivals: n.arrivals.clone(),
                };
                match audit(&course, &pt, d) {
                    Ok(()) => done.push(n),
                    Err(e) => {
                        lost_p += n.prob;
                        lost_lines += n.prob * n.t() as f64;
                        println!(
                            "not covered ({:.4}): [{}] booster audit: {e}",
                            n.prob,
                            n.labels.join(" ")
                        );
                    }
                }
            }
            Leaf::Lost { prob, at, why } => {
                lost_p += prob;
                lost_lines += prob * (at + ABORT_AFTER) as f64;
                println!("not covered ({prob:.4}): {why}");
            }
        }
    }
    if done.is_empty() {
        return Err("no path reached the finish".into());
    }

    // the plan: header as plan_from writes it, the root's bounce reaction from verify
    let paths: Vec<PathTape> = done
        .iter()
        .map(|n| PathTape {
            rows: n.rows.clone(),
            arrivals: n.arrivals.clone(),
        })
        .collect();
    let (commit, dirty) = etas::git_state();
    let mut header = tape.clone();
    header.rows.clear();
    header.arrivals.clear();
    let keep = ["source", "start", "start_tick"];
    let mut h: Vec<(String, String)> = vec![
        ("format".into(), plan::FORMAT.into()),
        ("account".into(), plan::ACCOUNT.into()),
        ("course".into(), course.name.clone()),
        ("course_sha256".into(), course.sha256.clone()),
        ("world_sha256".into(), course.world.content_sha256.clone()),
        ("diverge_tol".into(), format!("{:e}", plan::DIVERGE_TOL)),
        ("velocity_tol".into(), "1e-6".into()),
        ("objective".into(), OBJECTIVE.into()),
        (
            "solver".into(),
            format!(
                "alpine solve, commit {commit}{}",
                if dirty { " (dirty)" } else { "" }
            ),
        ),
    ];
    h.extend(
        tape.header
            .iter()
            .filter(|(k, _)| keep.contains(&k.as_str()) || k == "ticks")
            .cloned(),
    );
    header.header = h;
    let flat = plan::Plan::from_tape(Tape {
        rows: paths[0].rows.clone(),
        ..header.clone()
    });
    let r0 = verify::verify_path(
        &course,
        &world,
        verify::Plan::Reactive(&flat),
        &paths[0].arrivals,
    );
    let reacts: Vec<React> = r0
        .bounces
        .iter()
        .filter_map(|b| b.window_to_plan())
        .map(|(t0, t1)| React {
            t0,
            t1,
            event: Event::GlideOff,
            action: Action::PressJump,
        })
        .collect();
    let plan = plan::branched(&header, reacts, &paths)?;
    let text = plan.write();
    let back = plan::Plan::parse(&text)?;
    let out = dir.join("runs/solve.plan.tas");
    std::fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&out, &text).map_err(|e| format!("{}: {e}", out.display()))?;

    // every path: verify, the booster audit, ticks
    let results = plan::verify_paths(&course, &world, &back);
    let mut ok = results.len() == done.len();
    let mut by_ticks: BTreeMap<usize, f64> = BTreeMap::new();
    let (mut covered_p, mut exp_ticks, mut cov_lines) = (0.0, 0.0, 0.0);
    for (i, (path, r)) in results.iter().enumerate() {
        let n = done
            .iter()
            .find(|n| n.arrivals == path.arrivals)
            .ok_or("a plan path that no solved path has")?;
        let d = n
            .delays
            .iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap()
            .0;
        let pt = PathTape {
            rows: path.tape.rows.clone(),
            arrivals: path.arrivals.clone(),
        };
        let audit = audit(&course, &pt, d);
        let ticks = r.timer_tick().zip(r.finish_tick()).map(|(s, f)| f - s);
        let pass = r.ok() && audit.is_ok() && ticks.is_some();
        ok &= pass;
        if let Some(t) = ticks {
            *by_ticks.entry(t).or_default() += n.prob;
            exp_ticks += n.prob * t as f64;
        }
        covered_p += n.prob;
        cov_lines += n.prob * path.tape.rows.len() as f64;
        println!(
            "path {i} {}: p {:.4}, timer {} ticks (recorded {base_ticks}), secant {}, {} lines; {}{}",
            n.labels.join(" "),
            n.prob,
            ticks.map_or("-".into(), |t| t.to_string()),
            r.secant_seconds()
                .map_or("-".into(), |s| format!("{:.3}", s / 0.05)),
            path.tape.rows.len(),
            if pass { "PASS" } else { "FAIL" },
            match &audit {
                Ok(()) => String::new(),
                Err(e) => format!(" (booster audit: {e})"),
            }
        );
        for f in &r.failures {
            println!("    {f}");
        }
    }
    println!(
        "{}: {} segments, {} paths ({} to the end), {} tick lines; {:.1} s",
        out.display(),
        back.segments.len() + 1,
        back.path_count(),
        results.len(),
        back.tick_lines(),
        t_start.elapsed().as_secs_f64()
    );
    println!(
        "coverage: an attempt stays covered with p {covered_p:.4} (lost {lost_p:.4}); \
         covered timer ticks {:?}; expected over covered {:.1}",
        by_ticks
            .iter()
            .map(|(t, p)| format!("{t}:{p:.3}"))
            .collect::<Vec<_>>(),
        exp_ticks / covered_p
    );
    let per_run = (cov_lines + lost_lines + ATTEMPT_OVERHEAD as f64) / covered_p;
    println!(
        "expected ticks per completed run: {per_run:.0} ({:.1} s; attempts flown to the \
         finish or the abort, plus {ATTEMPT_OVERHEAD} per attempt for the reset and settle)",
        per_run * 0.05
    );
    Ok(ok)
}

/// `--scan`: the pilot's timer with each ring's held rotation moved, one ring at a time.
fn scan_held(mut ctx: Ctx, root: Node) -> Result<bool, String> {
    // the pilot path's timer (verify of its lines under its arrivals; the bounce's
    // missing reaction is the one failure expected, so the report isn't gated on)
    let timer = |ctx: &Ctx| -> Option<(usize, f64)> {
        *ctx.fallback.lock().unwrap() = vec![None; ctx.rings.len() + 1];
        ctx.advance(root.clone(), 0)
            .into_iter()
            .find_map(|l| match l {
                Leaf::Done(n) => {
                    let mut tp = ctx.rec.to_tape();
                    tp.rows = n.rows.clone();
                    tp.arrivals.clear();
                    let r = verify::verify_path(
                        ctx.course,
                        ctx.world,
                        verify::Plan::Tape(&tp),
                        &n.arrivals,
                    );
                    r.timer_tick()
                        .zip(r.finish_tick())
                        .map(|(s, f)| (f - s, n.prob))
                }
                _ => None,
            })
    };
    let base = timer(&ctx);
    println!("pilot, recorded held rotations: {base:?}");
    for k in 0..ctx.rings.len() {
        for (dy, dp) in [
            (-6.0, 0.0),
            (-3.0, 0.0),
            (3.0, 0.0),
            (6.0, 0.0),
            (0.0, -6.0),
            (0.0, -3.0),
            (0.0, 3.0),
            (0.0, 6.0),
        ] {
            ctx.rot_offset = vec![(0.0, 0.0); ctx.rings.len()];
            ctx.rot_offset[k] = (dy, dp);
            println!(
                "{} held yaw {dy:+} pitch {dp:+}: pilot timer and probability {:?}",
                ctx.rings[k].booster.name,
                timer(&ctx)
            );
        }
    }
    Ok(true)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("solve: {e}");
            ExitCode::from(2)
        }
    }
}
