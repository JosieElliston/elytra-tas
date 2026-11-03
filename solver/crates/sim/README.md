# sim

Flight kernels for the solver tools ([ARCHITECTURE.md](../../ARCHITECTURE.md), "Physics").
No globals: a kernel is a value. `sim::state` has the full state, the flight state
(position and velocity) and the rules between them: `FlightState::lift`,
`FullState::flight` (which says what normal flight is).

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
- **Ablations** are fields of `Hooks`; `LiftShape::Pow(k)` is the first (the
  conversion's lift factor). The default is the game.
- **Ghost points are corners, not segments**: a move can pass through a box thinner
  than the move with every point outside.
