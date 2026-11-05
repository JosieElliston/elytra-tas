//! Smoothing a generic plan at no cost in time: [`Rate`]-priced windows pinned at both ends.
//! why windows? on a course-long horizon a whole-run solve barely moves under the price (academy:
//! yaw travel 9%, against a third here), and pinned ends keep the ticks and the time.

use rayon::prelude::*;
use sim::generic::Generic;

use super::bounces::SCALE;
use super::ddp::{Goal, Opts, Plan, Problem, Rate, rotation};
use super::walls::Walls;

/// [`Rate::delta`] to take chatter out with, radians (5 degrees). why? chatter is smaller than
/// that, and a route's snaps are far bigger.
pub const DELTA: f64 = 0.087;
/// A round of [`smooth_pass`]es: window lengths and first cuts. why several? a cut's ticks only
/// move when another pass has them inside a window.
pub const WINDOWS: [(usize, usize); 6] = [(40, 0), (40, 20), (25, 12), (60, 30), (25, 0), (40, 10)];

/// One pass of windows of `w` ticks, the first cut at `off`, priced at `opts.rate`: each piece
/// flown between the plan's own states in the same ticks. A piece is kept if it solves at a lower
/// price, its clearance judged at `opts.margin - loose`. The last piece ends on the plan's last
/// state, so a finish crossed there is crossed the same. `opts.rate.from` prices the first tick.
///
/// The change into a piece and out of it are both priced, against its neighbors' rotations.
/// why alternate pieces? each then sees its neighbors' new rotations, not the ones they're
/// about to replace, or the cut keeps a kink.
///
/// Returns the plan and the pieces kept. Its states are the pieces', so the plan needn't be a
/// rollout of its controls under the generic kernel.
pub fn smooth_pass(
    walls: &Walls,
    plan: &Plan,
    w: usize,
    off: usize,
    opts: &Opts,
    loose: f64,
) -> (Plan, usize) {
    let n = plan.u.len();
    let mut cuts = vec![0];
    let mut t = if off == 0 { w } else { off };
    while t + w / 2 < n {
        cuts.push(t);
        t += w;
    }
    cuts.push(n);
    let mut out = plan.clone();
    let mut kept = 0;
    for color in 0..2 {
        let pieces: Vec<(usize, usize, Plan)> = cuts
            .par_windows(2)
            .enumerate()
            .filter(|(i, _)| i % 2 == color)
            .filter_map(|(_, c)| {
                let (a, b) = (c[0], c[1]);
                let from = if a > 0 {
                    Some(rotation(&out.x[a - 1], &out.u[a - 1]))
                } else {
                    opts.rate.from
                };
                let pb = Problem {
                    kernel: Generic::default(),
                    walls,
                    x0: out.x[a],
                    goal: Goal::Reach {
                        target: out.x[b],
                        scale: SCALE,
                    },
                    opts: Opts {
                        rate: Rate {
                            from,
                            to: (b < n).then(|| rotation(&out.x[b], &out.u[b])),
                            ..opts.rate
                        },
                        ..opts.clone()
                    },
                };
                let seed = &out.u[a..b];
                let s = pb.solve(seed);
                let lower =
                    pb.rate_price(&s.plan.x, &s.plan.u) < pb.rate_price(&out.x[a..=b], seed);
                let clear = Problem {
                    opts: Opts {
                        margin: opts.margin - loose,
                        ..pb.opts.clone()
                    },
                    ..pb.clone()
                }
                .feasible(&s.plan);
                (clear && lower).then_some((a, b, s.plan))
            })
            .collect();
        kept += pieces.len();
        for (a, b, p) in pieces {
            out.u[a..b].copy_from_slice(&p.u);
            // the end state is the pin; a solve only gets within its tolerance of it
            out.x[a + 1..b].copy_from_slice(&p.x[1..b - a]);
        }
    }
    (out, kept)
}
