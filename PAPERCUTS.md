# PAPERCUTS

missing tools / poorly configured tools / missing docs / general frictions / general suggestions

## From the first academy TAS (2026-11-04 23:45 EST)

### Solver

- **`Goal::Plane` can't express a finish the route passes twice.** Academy's finish plane
  (z = -99.7) is crossed the wrong way around tick 350, and `refit` cuts the horizon at the
  first tick past the plane, which is tick 1. Added `Goal::Gate`: rising crossings only, with a
  box on the state past it.
- **`Problem::feasible` is stricter than the solver can converge to.** It wants clearance at
  `margin - 1e-9`, and the augmented Lagrangian closes on the margin from below, often stopping
  at 0.01999. Good solves read as failures, and callers that gate on `s.feasible` (`reach`,
  `reach_min_ticks`) throw them away. `optimize` judges at `margin - 0.003` locally; probably
  belongs in the toolkit.
- **A failed stage looks like a slow one.** `stages` prints iterations, but 17 always means
  "mu ran from 1e-6 to 1e10 with no accepted step", i.e. converged or stuck, and 60 means the cap.
  Took a while to read. A stop reason on `Stage` would say it outright.
- **`refly::repair` doesn't scale to a course-long horizon.** It re-solves the whole remaining
  run on nearly every tick (the plan's active walls sit on the margin, so any drift counts as
  infeasible). That takes seconds a tick, and the re-solve, with the whole run's time to gain,
  wandered into a wall by tick 56. The open-loop plan actually flew clean with a 1e-2 gap. Added
  `refly::track` (short reaches to the plan's own states).
- **`LiftShape::Pow(0.99)` first made things worse here.** The recipe in TODO.md (smooth first,
  then vanilla) starts from a seed flown under different physics, so the recorded rotations drift
  into a wall in the first rollout and the ghost-wall solve stays stuck inside (clearance
  -0.6 at every price). It needs a contact-free smooth seed first, or continuation in k.
- **`stretch.rs` and `bounces.rs` each have their own `state_at`, `flight`, `solve_covered`,
  `walls_for`**, and burrowed grove's `resolve.rs` a third `solve_covered`. `reach.rs` a fourth.
  Time to extract them.
- **No toolkit "sliding windows" tool.** `optimize.rs`'s reach/pass/roll is the thing that worked,
  and it owes nothing to academy (only the gate does). It should move to `etas::solve` for the next
  map.
- **`Walls::around` over a whole course is one big bounding box** (about 15M cells of `Vec`s on academy).
  Fine on this machine, wasteful: a route is a thin tube.

### Seeds and the start

- **Seed tapes don't all start at the reset pose,** and nothing says so. `flights` prints PASS for a
  tape starting on a ledge (-25.6, 283.5, 227.4), or even at (53, -13, -139), which `/etas play`
  can't start from: it resets to (-26.5, 282, 210.5). The fastest seeds are of this kind. `flights`
  could print "starts at reset: yes/no", and `segments.py --tapes` could cut tapes at the reset.
- **`verify` says `not from the course start` for a flight trajectory, and still PASS.** Correct for
  a flight, but it's easy to think a PASS means flyable.
- **Timer-start pre-drop is free time and nothing optimizes it.** The start area is an 83-block
  chimney whose time doesn't count, so the exit speed there is pure gain: the objective should be
  finish minus exit, and `Goal` only has terminal costs. Top exit speeds in `splits.md` are remote
  and rough, but deftenvy's 1.65 and lqmbda's 1.80 against rambells' 2.2-3.0 suggest ticks there.

### Docs and tools

- **`etas view` has no way to compare time.** The runs are drawn with dots every 10 ticks, but
  which run is ahead where needs a chart of time against arc length. Would have been the first
  thing to look at for "where did the 76 ticks come from".
- **No tool to diff two runs' inputs or path** (distance from the recorded path, speed, roughness).
  Wrote it inline in Python twice.
- **Trajectory files are 3 lines of header then rows**, and every ad hoc reader has to know it.
  A tiny `tools/traj.py` would save it.
- **`runs/` is gitignored, so the first TAS isn't in the repo.** Fine for bulk, but a finished plan
  seems worth keeping somewhere tracked.

### Left open by this TAS

- **Two seeds never got a first plan** (41.64, 42.82): a wall hit's state no reach gets to, and
  merging pieces until the tail still fails. Their first plan should fly around the hit, not
  reach the state after it.
- **`track` hits a wall on one seed at every window** (30.98, plan 541, tick 364 at windows 24
  through 3), though the same plan's generic clearance is fine. Not understood. It costs
  nothing today, as another seed is ahead, but a best plan could die this way.
- **Plans ride assumed faces.** The 540 finishes at x = -6.513 against an assumed x_min of -6.521,
  and never enters the `inner` box. It's the solver's job to use all of a box, so assumed faces
  need a price or a margin, or a tentative finish becomes a failed run. Refit, then re-solve the end.
- **Clearance at the end of the plan is 0.004** (the tracker's margin), over the floor at y = -57.
  Real playback has no such drift, but the ground kill rule is provisional. `TRACK_MARGIN` and
  `KEEP` are guesses.
- **`verify` says `not from the course start` on a plan that starts at the reset pose,** because its
  first ticks differ from `course.start.inputs`. Compare the pose, not the inputs.
- **No prefix without a floor bounce.** Both tapes that start at reset and replay land on the start
  floor and jump. It's before the timer and has an `@react`, but it's a glide-off playback has
  never handled on EMC, right at the start of the first real flight.
- **Inputs chatter.** Roughness yaw d2 max 254 degrees, 82 pitch reversals. Playback doesn't care;
  whether EMC does is unknown. The `Pow(0.99)` recipe could be retried from the optimized plan,
  which is contact-free.
- **Seeds cross-pollinating is untried.** Results run from 540 to 586 by seed, and each lands in
  its own local optimum: per-stretch best of all the optimized runs could be spliced, as gates
  (corner-time) would price them. Probably the next few ticks, with the exit speed above.
- **`solver/courses/academy/runs/optimize/all.log` is the only record of a batch.** `optimize` should
  write a results table.
