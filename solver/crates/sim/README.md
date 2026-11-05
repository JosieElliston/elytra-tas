# sim

Flight kernels for the solver tools ([ARCHITECTURE.md](../../ARCHITECTURE.md), "Physics").
No globals: a kernel is a value. `sim::state` has the full state, the flight state
(position and velocity) and the rules between them: `FlightState::lift`,
`FullState::flight` (which says what normal flight is); and `GlideState`, a glide
that may be on the ground, for perfect ebounces. `FullState` is what the faithful
kernel's full mode steps.

## Faithful kernel (`sim::faithful`)

`Faithful::step` is one normal-flight tick against a `world::World`, with the game's
arithmetic (float rotations, `Mth`'s sine table and fdlibm's `Math.cos` from
`port::mth`, the only thing taken from port) and vanilla collision: the L-shaped
move, clipped axes' velocity zeroed, `horizontalCollision` with its 1e-5 tolerance.
A wall or ceiling hit is a `Contact` with the predicted fly-into-wall damage. Anything
else ends the step with an `Event`: landing, a climbable at the position, fluid,
stuck and bouncing blocks, unknown space, or the box inside a block. `Faithful::look`
holds the rotation terms; reuse it across velocities.

- **Exact against port** (bit for bit, this machine, as of 2026-11-03 01:05 EST):
  all 690 normal-flight ticks of the Burrowed Grove tape against its world fixture,
  and the landing at 712 (`tests/faithful_tape.rs`); ~290k random ticks in box
  worlds per run of `tests/faithful_props.rs` (planes and scattered blocks of two
  dozen shapes, starts snapped to within a hair of grid lines, rollouts of 40
  ticks), where every event matches port's (`agree` in `tests/common/mod.rs` says
  what matches).
- **Collision is against boxes, not the game's `VoxelShape` cells.** They differ
  only when the box is already inside a block's box and the shape has a grid line
  ahead of the face; the step raises `InsideBlock` there instead of guessing. why
  not cells? the grid comes from `VoxelShape.optimize`'s merging, which would be
  port's shapes code again. A flight that starts outside every box reaches it only
  through the 1e-7 margins' rounding (not once in the tests' 20k rollouts per
  run). Starts inside a box with no line in the way fly exactly.
- **Unknown space is what the step reads**: the move's collision cursor, the fluid,
  stuck and climbable cells. Port also reads the 1.8-tall standing box for its crouch
  flag, so near the top of a world's known region port fails where faithful flies.
- **On a landing the step stops at the clip.** If the move also clipped
  horizontally, the game tries a step-up, so the end-of-move checks (fluid, bounce,
  stuck) are skipped and the event is `Landing`.
- **Speed** (M5, release, tape ticks from port's states, 2026-11-03 01:05 EST):
  `Faithful::step` 1.15e6 ticks/s, `port::tick` 4.9e4. Nothing is optimized yet:
  every tick looks up each cell of its collision cursor and builds its boxes.
  `cargo test -p sim --release --test faithful_tape bench -- --ignored --nocapture`.
- **Lift and project**: `lift` fills what normal flight doesn't fix with neutral
  values (no sprint, fall distance 0, no keys). They never change a normal-flight
  move but matter after it (sprint at a jump, fall distance at a landing), so a
  recorded run resumes from its recorded full state, not a lift. The tape test checks
  that port flies a lift exactly as the recorded state.
- **Perfect ebounces: `Faithful::step_glide`**, from a `GlideState` with the tick's
  keys. The client never ends a glide, so after a landing it glides on: on the ground
  the elytra update runs and the floor clips the move again, and held jump jumps
  (`jumpFromGround`, sprint boost along the yaw) before the elytra update. Port does
  the same, so its replay of a bounce is the perfect one. In the air it is `step`'s
  arithmetic; a landing is a tick with `landed` set (with step-up, which `step` stops
  short of), not an event. Exact against port on the glide state (2026-11-03 06:00
  EST): ~340k ticks per run of `tests/faithful_ebounce.rs` (floors of 17 shapes with
  bumps, ceilings and event blocks; ~16k landings, ~12k jumps, ~2.5k with sprint,
  ~1k held back by the jump delay, ~500 step-ups), and the 2026-10-19 ebounce
  replay's first bounce (`etas` `tests/ebounce.rs`). Sneak is not modeled.
- **Projection for bounces.** `GlideState::flight()` is the flight state once off the
  ground, and `step` flies it the same (the test checks ~280k ticks). It drops what
  the next bounce reads: sprint (which only ends while gliding, by releasing forward
  or a non-minor wall hit), the jump delay (10 after a jump, 0 after a tick without
  jump) and the last keys. So carry the `GlideState` through a flight that bounces
  again; `FlightState::lift_glide` fills them neutral (no sprint boost).

- **The full mode: `Faithful::step_full`**, any client tick from a `FullState` and
  the tick's keys: ticks without the glide (walking with the ground's friction and
  speed factor, falling, jumping with the sprint boost, air control, the sprint
  key (and the double tap, which never fires at sprint window 0), sneak backing off
  ledges, crouching under a ceiling),
  the jump edge that starts a glide, gliding in any pose (the standing box on the
  tick after the server's glide-off), and the pose update. The server's packets go
  on the state between ticks: `FullState::apply_glide_off(Faithful::server_pose(..))`
  and `apply_velocity`. It is port's `LocalPlayer.tick` in this kernel's terms, the
  other modes' collision and block checks for the pose's box. Exact against port,
  full state included (2026-11-03 10:57 EST, `tests/faithful_walk.rs`): every tick
  of the two recorded runs with server events in `etas`'s `tests/data`, from port's
  state and flown alone (ebounce run 1: 295 ticks, 4 glide-offs, 5 glide starts, 6
  ticks without the glide after the deploy, a sneak tap; alpine run 0: 1520 ticks,
  1058 on the ground, 8 booster and echo velocities), and ~45k random ticks per run
  over box worlds (holes, walls, low ceilings, slabs, ice, soul sand, honey,
  fences; ~9.5k on the ground, ~1.4k of them sneaking, ~2.5k wall contacts, ~2.7k
  jumps). Not optimized: it recomputes the pose fits every tick.

## Generic kernel (`sim::generic`)

`Generic::step` is one contact-free gliding tick (`LivingEntity.aiStep`'s small-velocity
zeroing, `updateFallFlyingMovement`, `pos += vel`) as plain math over `Scalar`: `f64`,
`f32`, or any `Copy` num-dual type (`Dual64`, `HyperDual64`, `Dual2Vec` via
`num_dual::hessian`, ...). `Generic::look` holds the rotation terms, so reuse it when
one input feeds several velocities. Inputs are degrees, as the game holds them.
`generic::ghost` has the geometry for the `Ghost` collision model: signed distance from
the gliding box to `world::Aabb`s, and the points of the L-shaped move.

- **Gap from the game, Burrowed Grove tape** (689 contact-free gliding ticks, as of
  2026-11-02 21:51 EST): next pose off by max 1.9e-5, median 4.5e-6 on `f64`, about the
  same on `f32`. The game's sine table in the look vector is most of it. `cargo test -p
  sim --test generic_tape -- --nocapture` prints it; the test bounds it at 1e-4.
- **Zeroing is on by default** (`zero_small_velocity`). why? ticks 555–559 on the tape
  are it, and they miss by 4.5e-4 or more without it. It is a jump: zero derivative
  inside the band, none at the threshold.
- **Pitch -90 is a dead tick**, as in the game: no conversion, nose-up or turning.
  +90 turns, as the game's does. Bound pitch above -90 in a solver, or meet a cliff.
  The other non-smooth points are listed in `generic/mod.rs`.
- **Flight only.** It has no ground, no keys and no glide-off: a solver that needs
  a lost glide or a walk uses the faithful full mode. Making it cover them would
  take `travelInAir` (the input along the yaw, gravity after the move, friction),
  the jump with its sprint boost, and the sprint and crouch bookkeeping, all smooth
  except where the game branches (on the ground or not, jump delay, sneak at
  ledges), plus a collision model that keeps `onGround`; the branches would need
  the same care as the zeroing below.
- **Ablations** are fields of `Hooks`; `LiftShape::Pow(k)` is the first (the
  conversion's lift factor). The default is the game.
- **Ghost points are corners, not segments**: a move can pass through a box thinner
  than the move with every point outside. `ghost::leg_sdf` measures a whole leg of
  the L instead, exactly (a leg moves along one axis, so its sweep is a box); the
  `etas::solve` DDP constrains that.
