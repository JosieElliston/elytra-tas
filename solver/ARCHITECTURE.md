# solver architecture

How we get the min-time plans that the `/etas` mod flies on EMC. This is a toolkit, not
one solver. A course's plan comes from a short program written for that course, which
composes tools from here (cut the course, seed, solve, stitch, finishing pass), and the
plan is only trusted once `verify` replays it. why? we don't know ahead of time which
methods, cuts or objectives a given map needs, and elytrasim showed what happens when
each experiment owns its own copy of the problem: nothing composes. So the shared part
is kept small and strict (physics, world, course, artifacts, verify), and everything
else is a tool that may come and go.

Status as of 2026-11-04 19:40 EST: build-order steps 1–7 exist; nothing has flown on EMC.
- Steps 1–4 work end to end on Burrowed Grove: port, world, course, and `verify` of the
  recorded tape. The client-gametest oracle runs, and port matches it on all but 3 of
  9455 ticks (the JVM's `Math.cos`, see `crates/port/README.md`).
- Kernels: the faithful kernel flies any tick, with or without the glide, bit-exact
  against port (`step_full`). The generic kernel covers flight.
- Solvers (`crates/etas/src/solve/`): DDP through ghost walls re-solves stretches between
  cuts. Burrowed Grove goes from 647 to 615 ticks, though it chatters (no roughness price
  yet). Super classic downtown goes from 124 to 119 timed ticks, seeded from a recorded
  run with lost glides, with a branched plan of 4 paths. Alpine has a branched plan over
  its boosters whose coverage and expected time haven't been assessed.
- Plans (PLAN-FORMAT.md, version 2 with branches) are checked path by path by `etas
  verify` and played by `/etas play` (mod 0.5.0), so far only in singleplayer gametests
  and only with plans the gametests wrote.
- Recorded runs carry what the server did as `@server` events and replay under port. A
  velocity the model doesn't explain is just such an event, so a recording that took
  damage replays and verifies. `check_runs.py` sweeps the replays for runs that don't;
  it hasn't been run to completion.
- Academy (`courses/academy`, phases `flights`, `optimize`, `tas`) has a whole-run plan from
  the reset pose at 540 timed ticks, from recordings of 622-852, verified but unflown
  (2026-11-04 23:45 EST). Sliding min-tick windows did nearly all of it: whole-run DDP alone stalls after
  about 3 ticks. Its machinery is in the course crate so far.
- Not yet: a solver-written plan flown by the mod, any playback on EMC, plans that take
  damage on purpose, fireworks.

## Layout

```
solver/                     cargo workspace
  crates/world/             world file reader: blocks, collision boxes, movement effects,
                            unknown space; shared by port, sim and etas
  crates/port/              mechanical port of MC 26.2 movement, for fuzzing and verify
  crates/sim/               faithful, generic and specialized kernels, collision models,
                            world queries
  crates/view/              renders the world and annotations to images; knows only `world`
  crates/etas/              the toolkit: problems, solvers, seeders, transforms, analysis,
                            artifacts, verify; plus the `etas` CLI for the generic tools
  courses/<name>/           one crate per course: its phases as binaries, its runs/
  oracle/                   Fabric client gametest that produces Minecraft's own trajectories
```

Python stays where the work is empirical: `world.py` and `flashback.py` for reading
replays, and course-boundary fitting from logs. Python writes the world and course
files; Rust reads them. Pictures of runs in the world are Rust (`view`, `etas::draw`,
`etas view`), since course programs want them inline; charts of values over ticks are
Python, off artifact files.

## Physics: written several times, on purpose

Four kinds of implementation, each with its own job. They don't share code, and they
don't have to share structure or API. Only `port` mirrors Minecraft's.

1. **`port`: the mechanical port.** Transcribed with Minecraft's own casts (float
   rotations, `Mth.sin` from the table where the Java uses it, `Math.cos` on a double for
   the lift, `0.99f` widened to double, and so on), its state as Minecraft's fields, its
   call structure as Minecraft's. Nothing clever and nothing reorganized. why? it is the
   thing we check against the game, so the only question it has to answer is "is this
   the Java", and every deviation makes that harder to read off. Its API is whatever
   that structure gives; kernels don't have to match it. Scope, client side:
   - `LocalPlayer.aiStep`: the jump edge, `tryToStartFallFlying`, sprint,
     `noJumpDelay`.
   - `LivingEntity.aiStep`: among other things it zeroes `v_y` when `|v_y| < 0.003` and
     the horizontal velocity when it is tiny, before `travel`. why? every pull-up apex
     goes through it; it is what the extractor's `glide_mismatch` ticks 555–559 on the
     Burrowed Grove tape are, not wall contacts.
   - `LivingEntity.travel` (elytra and ground paths), `jumpFromGround`.
   - `Entity.move` with `VoxelShape` collision and step-up. why step-up? starts are
     full of slab boosts.
2. **Faithful kernel.** The game's arithmetic, casts and sine table included, but with
   our own state, our own split between flight and collision, and the API every solver
   sees. It may be optimized (rotation-dependent terms computed once per input,
   collision against a precomputed box list, nothing recomputed), but that can wait:
   the first version can be straightforward. Vanilla collision only. why? it is the
   game; Secant and Ghost belong to the generic kernel. For solvers that need the exact
   game: finishing passes, a DP that flies exact steps.
3. **Generic kernel.** Plain math over a float trait, with no casts and no sine table,
   which is what lets it be simpler and faster than the faithful one. On `f64` it is the
   precise physics: more accurate than the game, not a copy of it. On dual and hyperdual
   numbers ([`num-dual`](https://crates.io/crates/num-dual) is the candidate) it gives
   gradients and Hessians, which the table would make zero almost everywhere. On `f32`
   it is a fast approximate kernel, not a faithful one (Minecraft mixes float and
   double). Most solvers use this one. Its gap from faithful is measured and reported,
   and also bounded loosely on a fixed regression set (the recorded runs). why the bound?
   so a sign error fails a test instead of showing up as a slightly worse number.
   Whether it models the small-velocity zeroing above is its choice; if it doesn't, the
   gap report says so.
4. **Specialized kernels.** Algebraically collapsed or otherwise rewritten forms, for
   speed in a particular solver. Each states a tolerance against the faithful or the
   generic `f64` kernel, whichever it approximates, and tests it.

Turning a generic-kernel solution into one that holds under the faithful kernel
(clearances that close up, a finish that slips a tick) is the course program's job, not
something the toolkit enforces. `verify` is the only gate.

**Ablations** are hooks at named sites in the generic kernel only. Two are planned; the
first is the lift shape: the default leaves it alone, the curvature ablation uses
`lift.powf(k)` in that one expression, the down-to-forward conversion
`v_y * -0.1 * lift`, and plain `lift` in the gravity term (at k = 0.99 the optimum stops
chattering; elytrasim's `pumpdp --conv-lift-exp`). Keep
the hook list short and named; a new ablation is a new hook, not a flag threaded through
the kernel. The faithful kernel has none: it is the game.

There are no globals. The kernel is a type parameter or a value passed in. why?
elytrasim's process-wide trig and flight-mode switches mean two kernels can't run side
by side, and a forgotten `set_*` call silently changes every result after it.

**Collision models** (generic kernel; faithful has Vanilla only):

- `Vanilla`: the L-shaped move, resolved y first, then the larger horizontal axis, with
  the clipped axis' velocity zeroed, and step-up on ground. What the game does.
- `Secant`: the segment between tick positions, for when the L's sub-tick detail costs
  more than it is worth.
- `Ghost`: no collision; the signed distance of each point the move visits, for
  constrained optimizers (augmented Lagrangian through the walls).

Contact-free is the normal case. Deliberate contact without damage is in scope. The
constraint is vanilla's: damage is `10 (h_before - h_after) - 3` when positive, with
`h_before` taken before the elytra update, so drag counts toward the loss, and it is the
server's copy of the physics that judges it. Contact with damage is later (see
Mechanics).

### State

Two levels:

- **Full state**: every field the port reads, so a cut anywhere can be resumed:
  position, velocity, onGround, glide flag, pose, sprinting, `wasJumping`,
  `noJumpDelay`, the supporting block, the collision flags that stop sprint, and
  whatever else the port turns out to read.
- **Flight state**: position and velocity, for normal flight (gliding, airborne, nothing
  else pending). It lifts into a full state with the fields that normal flight implies,
  and a full state projects onto it only when it is in normal flight. Most solvers work
  here. why? carrying ten ground fields through a DDP that never touches the ground is
  cost with no use.

### Testing the physics

The standard is faithfulness to Minecraft, not bit identity: Minecraft isn't
bit-identical to itself (`Math.cos` may differ in the last place between JVMs and
platforms), so a mismatch at that level is noise, and anything larger is a bug. Each
layer is checked against the one below it, never against elytrasim:

- **`oracle` against Minecraft.** A Fabric *client* gametest (`runClientGameTest`, as
  undeploy's tests do), not a server one. why? a server mock player skips
  `LocalPlayer.aiStep` (jump edges, glide start), takes wall damage, and loses the
  glide on the first ground tick, so it can never produce the states ebounces and glide
  starts depend on. It drives the player through the same input path the `/etas` mod's
  playback uses, builds random block setups, feeds random rotations and keys for a few
  hundred ticks, and logs every tick's full state. The server's glide-off is held or
  forced as undeploy's gametest does. Seeds are recorded, so a case reruns; logs go to
  `oracle/cases/`.
- **`port` against real play first,** before the oracle exists: the Burrowed Grove tape
  and the logger's client logs are real client data already.
- **Faithful against `port`.** Property tests over random states, inputs and worlds.
- **Generic and specialized kernels,** each against its reference to its stated
  tolerance.

## World and course: what we know exactly, and what we measured

Two files, written by Python, read by Rust.

- **World**: block states in the course's bounds and their collision boxes, from the
  chunk packets in a Flashback replay (`world.py`, `extract_world.py`, whose docstring
  is the format) and the block table `data/blocks-26.2.tsv.gz`. It carries its source
  replay, a content hash and the hash of the block table it was written against.
  - **Unknown space is explicit.** A chunk the replay never loaded is unknown, not air,
    and a move whose box enters unknown space fails `verify`. why? otherwise a route
    through an unloaded wall would verify. Block updates are applied in stream order.
  - **What isn't exact says so.** `min_y` comes from the replay's dimension-type
    registry data when present; the file records how it was found, and a guess (the
    ground-pose vote) is flagged.
  - **Blocks carry a movement-effect class**, not only boxes: climbable, fluid, speed
    factor, bounce, stuck. Kernels either model a class or treat entering it as an
    event. For now climbable blocks (which end the glide) count as killing you, and the
    rest only need to be expressible, since they rarely appear on maps. Climbing comes
    from the server's tags, which the world file carries.
  - **Position-dependent shapes** (bamboo's offset) are stored un-offset with their
    offset type, and the offset is computed from the position.
  - **Coverage is reported:** chunks not loaded, updates applied or dropped, entities
    with collision (boats, shulkers), which are not in chunk data and not in the file.
- **Course**: the EMC rules for one map, fitted from logs and replays. The world file it
  refers to (by hash), the start, where the timer starts, the finish, where touching the
  ground is allowed (ebounces) and where it kills, and later boosters. Rules are
  predicates over what the server observes, not bare regions. why? the timer started
  when landing at y = 167 but not after 300 ms airborne below y = 168. The
  ground-allowed region and the timer-start region are separate fields, even though
  they are often the same. why? secrets you can stand on, and odd start-area behavior,
  have been seen. Versioned: a course file states its format version and the logs it
  was fitted from.

## Artifacts

Self-describing files that tools pass around. Each has a header naming its course and
world hashes, the kernel (and any ablation) it was produced under, the objective by
name, and the git commit of the code that defined that objective, marked dirty if the
tree was. The list below is what we know we need. Add types when a tool needs one,
carefully: a new type is a new thing every later tool may have to read.

- **Trajectory**: an initial full state, then per tick the input (yaw, pitch, keys) and
  the state after the tick, with the `.tas` tape's convention for which tick a row
  belongs to. The common currency between tools.
- **Plan**: what the mod flies, start to finish. Read by the `/etas` mod in Java, so the
  format is line-based text like `.tas` tapes, trivial to parse there; a plan with no
  branches is exactly a `.tas` tape, which `tas_play` can already fly.
  - **Start contract**: how the start pose is reached, and the bit-exact pose, rotation
    and keys at tick 0.
  - **Segments**: a tape of exact float bits and keys per tick, plus the expected
    position, so the mod can tell when it has diverged.
  - **Reactive keys**: an input tied to an event rather than a tick ("press jump on the
    tick the undeploy arrives"). Perfect ebounces need this from the start.
  - **Branch nodes**: an observation (which packet, the tick it was handled, its
    payload) maps to a child segment; an unlisted observation is an abort leaf. why the
    payload? a booster's velocity comes from the look the server last received, so the
    same arrival tick can carry different velocities. Children may rejoin (a DAG), so a
    booster that re-sends every tick doesn't multiply branches.
  - **Playback** checks that the account is `deftenvy` before moving the player (see
    AGENTS.md).

Divergence aborting is not a no-lag claim: gliding is simulated on the client, so an
open-loop run flies through server lag unchanged, while the timer is wall-clock time
between server ticks. A no-lag claim comes from the logger's server timing over the timed
span.

Objectives are code, not data. The library provides terms (raw tick of entry, secant
time, clearance, roughness price, the value read off a cut), and each phase of a course
writes its own objective in Rust. why? a segment's objective is often ad hoc for that
phase of that map, and no schema anticipates it. The cost is that an artifact can't be
re-checked for optimality from the file alone, so `verify` checks only physical claims.

## Verify

`etas verify FILE` replays a trajectory or every path through a plan with `port` and the
course's world and rules. It reports:

- the finish tick and the secant time, as predictions of EMC's timer, which is
  wall-clock time between server ticks over fitted start and finish rules
- contacts, with predicted damage for each
- ground touches, and whether each was allowed
- entries into unknown space or killing blocks
- clearance to the nearest wall
- chatter and curvature figures

It fails if the file's recorded states don't reproduce. Nothing goes to the mod without
passing it. why? the judge should be the least clever code we have, and a solver's own
score is computed under whatever kernel, collision model and objective that solver
chose.

## Tools

All in `crates/etas`, as library functions; the ones useful without course-specific code
are also `etas` subcommands. Expected to grow and change; none is special. Shared
interfaces between them are extracted once two course phases actually compose, not
before. why? shooting, DDP and the tunnel DP need different state and terminal values
(corner-time), and a premature common interface becomes an adapter layer.

- **Seeders**: from a `.tas` tape (a recorded run, flyable by construction), from a tape's
  flight alone (`flight_from_tape`), from a polyline sketch, from a start state.
- **Solvers**: direct shooting, DDP through ghost walls under an augmented Lagrangian,
  the tunnel DP, multistart wrappers, later route search.
- **Problem pieces**: entry state or set of entry cases, exit condition (crossing a
  region, a state constraint, a value function from the next segment), objective terms,
  constraints (no ground, no damage, clearance), collision model, kernel and ablation.
- **Transforms**: cut a trajectory at a tick or region, stitch segments, splice in a
  re-solved stretch, run a finishing pass over a stitched whole, re-fly a generic
  solution under the faithful kernel and report where it parts.
- **Analysis**: flyability (curvature, chatter), ablation comparisons (what chatter buys:
  the same problem under the generic kernel with and without the ablation, or with and
  without a roughness price).
- **Pictures**: `etas view` for a contact sheet of runs; `view` and `etas::draw` from a
  course program for anything else. Primitives first, helpers are plain functions over
  them. why a CPU ray tracer? clips, masks and orthographic views are simple ray and cell
  tests, with no meshing and no graphics stack, and it's fast enough to not matter so far.

Course crates are where composition happens. A typical one: phase 1 flies the start
(a recorded prefix, or ebounces) to the exit of the start area, the usual cut; later
phases solve stretches between cuts against the next segment's value or a rough
reference; a last pass stitches and refines; `verify`; emit the plan.

## Mechanics, in order

1. Collision without damage, no ground inside the course, climbable blocks kill. Needed
   on every map. Maps without ebounces start from a recorded prefix: Josie's inputs up
   to the deploy, taken from a replay, and the solver takes over from there.
2. Perfect ebounces. Pressing jump on the tick the undeploy arrives flies like a bounce
   that never lost the glide, so the solver models a perfect bounce and no delay;
   making it so is the mod's job, through a reactive key. Most map starts need this.
3. Boosters. The first event whose observation (arrival tick and payload) makes plans
   branch, and the first that needs a check against recorded server behavior.
4. Collision with damage. Recordings that take damage are in model: the server's velocity
   overwrite is recorded as an out-of-model velocity event. Plans can't take damage yet.
   why not model it? the server computes it on its own copy of the physics, which
   disagrees slightly, and it arrives a few ticks late at a time that varies.
5. Fireworks, in the single-rocket mode.
6. Imperfect ebounces.

Boosters (`crates/etas/src/booster.rs`) and imperfect ebounces are built: a booster's or
a lost glide's arrival is a plan branch. They don't share one shape (a booster's payload
depends on the uplink, damage on the server's simulation, an imperfect bounce on
neither), so each kind of event gets its own handling.

## Build order

1. `port` with the scope above, tested against the Burrowed Grove tape and the logger's
   client logs.
2. World writer and reader: movement-effect classes, explicit unknown space, `min_y`
   provenance.
3. A Burrowed Grove course file, with the finish and timer-start rules fitted from logs.
4. `verify`, with the plan as a `.tas` tape, flown by `tas_play`.
5. The faithful kernel, the generic kernel and one solver, seeded from the tape: a first
   course crate that re-solves stretches of Burrowed Grove and verifies the result.
6. The client-gametest oracle for collision fuzzing; then `/etas` playback in Java, with
   reactive keys and ebounce starts.
7. Branching plans and boosters.

## Compute

Local (M5) or the cluster, the same binaries: a course phase is a binary run with file
arguments, parallel with rayon inside. Sweeps over phases or seeds are shell or sbatch
scripts in the course crate.

## Open

- The timer's start and finish offsets against ping: predicted and shown times differ
  by up to 0.14 s on super classic downtown, cause not settled.
- The second curvature ablation's hook.
- Route search: what proposes routes when no recording exists.
