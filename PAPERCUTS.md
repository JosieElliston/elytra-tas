# PAPERCUTS

missing tools / poorly configured tools / missing docs / general frictions / general suggestions

## From the first academy TAS (2026-11-05)

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
- **`Walls::around` over a whole course is one big bounding box** (16M cells of `Vec`s on academy).
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

- **A PASS doesn't say which glide-off delays a plan survives.** `etas verify` passed the first
  plan with `@react 24 29` for a landing at 21, and a singleplayer glide-off 2 ticks after the landing
  aborted it on line 23 (2026-11-05 00:27 EST; other singleplayer logs show 1 to 3 ticks, or none). `verify` could print "delays covered: 3..8, aborts on 2" beside
  the PASS. To test a plan's main line there, `/etas play hold_glide=1`.

### Smoothing and routes (2026-11-05)

- **A whole-run solve takes a rate price poorly.** On academy, weight 1 cut yaw travel 9% (2544 to
  2325) and pitch 13%, then lower weights moved it about 1% more, most stages accepting no step.
  The same price on windows pinned at both ends cut both about a third, at no cost in time.
  `optimize --relax` uses windows for this reason.
- **A reach pays its end state's error in its last controls.** Flown as is, each end is a kink: the
  tracker's windows every 24 ticks, smoothing cuts, and a join's end (pitch 27, 24, 0, 17 on academy,
  2026-11-05). `track` now flies only the first half of each solve, and `smooth_pass` prices the
  change out of a window too (`Rate::to`). Other callers of `Goal::Reach` still need to think of it.
- **A server hit left in a window blocks all smoothing over it.** Academy's tail ended on the floor
  heading down, so every window over it was rejected, which is what left its approach unsmoothed
  (2026-11-05). `tas` lets go of the last state's velocity for this. `smooth_pass` could report why it
  rejects a window: finding this took a debug print.
- **Some reaches stop short of their pin by about 3e-4 blocks, whatever the schedule.** Restarts and
  other prices land on the same point, under `Opts::tol`'s 1e-6 (2026-11-05), so a good solve reads as
  a failure. `tas`'s server windows accept 1e-3 and let the tracker take it up. Why they stop is
  unknown.
- **`verify` doesn't model the server's copy,** so a plan can PASS and be thrown off by a wall only the
  server hits. `tas` checks this itself, for `p_t + v_t` and `p_t + v_(t-1)`.
- **Academy's route can't hold the tick-behind box at t267, at 537 ticks.** It goes through a slot
  between a pillar (x 13-14, z 6-7) and a wall (z 9-10), and that box ends in the pillar under either
  axis order. Repair windows up to 350 ticks stopped 0.11 blocks short, and `tas` now exits 1 on this
  route, with a plan that also hits on `p_t + v_t` (2026-11-05). So `runs/tas/academy.tas`, as flown
  (17 of 25, 26.84-26.86 s), is from f2dd3cf, which holds only `p_t + v_t`. Not proof: nothing tried
  spending ticks or another route.
- **A seed inside a wall stays there.** Pure pursuit seeds along a route all failed at clearance -0.2 to
  -0.7, whatever the price (2026-11-05), so `routes` uses `guided`, which looks ahead and won't hit
  anything. A solve's `Stage` still doesn't say it's stuck.
- **A table's read doesn't rank flights.** Academy's forecast best polished to 552 and one near its worst
  to 540, and a stitched flight's own time, 620 to 656, doesn't either (2026-11-05). `optimize --hybrid`
  takes 2 to 5 minutes a flight, so polish and compare. Tables gained about 2 ticks over none, for hours
  of compute.
- **`first_plan` can't always seed a splice.** 12 of 15 route hybrids failed with `last piece:
  clearance -0.5`, since a failed piece merges forward until the whole tail is one solve. So those
  routes are untested, not slower: both z = 178 x = -23 classes among them.
- **`verify` can't read a spliced trajectory,** as it replays the rows and stops at the splice.
  `optimize --hybrid` scans for the timer and finish by position instead.
- **A cross section can't tell the sides of a free-hanging obstacle.** `route_hybrids.py` labels
  openings, which separates the hall's windows, but the candle room is one component at every
  plane, so it falls back on bands of x and y.
- **Remote attempts are sampled about every other tick.** Counting their rows halves their time,
  and their exit speeds are noisy. Use `t`, and compare with local attempts first.
- **Yaw flips at pitch 89 survive smoothing.** A window's warm start stays on its branch. Reseeding
  with a steady turn either way ended inside a wall or solved no smoother, at four flips tried
  (2026-11-05, plan ticks 109, 246, 322, 329). Not proof they're optimal.
- **A join can hinge on tiny changes to its seed.** With the same flight and prefix, a seed capped
  at pitch 89 joins and an uncapped one finds nothing, under either bound (2026-11-05). It looks
  like a bad pitch bound, and at first I blamed the bound. `tas` caps its seeds at 89, which may be
  luck, as 89.5 fails too. A solve's `Stage` still doesn't say why it failed.
- **A tape's walk to the back ledge doesn't replay under port.** `36.440_..._run3` is at y 262 by
  tick 91 where the recording says 282.8, so no tape gets a plan from the reset to the z = 227 ledge.
  An ideal drop off that ledge, seeded from the flight, reached 539 against the plan's 537
  (2026-11-05), so a hand-written walk wasn't worth it then. That search isn't exhaustive: 11 of 84
  tries solved.

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
