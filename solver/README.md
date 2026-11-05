# solver

How we get the min-time plans that `/etas play` flies on EMC. This is a toolkit, not one
solver: a course's plan comes from a short program written for that course, which composes
tools from here, and the plan is trusted only once `etas verify` replays it. why? we don't
know ahead of time which methods, cuts or objectives a map needs, and elytrasim showed what
happens when each experiment owns its own copy of the problem: nothing composes. So the
shared part stays small and strict (physics, world, course, artifacts, verify), and
everything else is a tool that may come and go.

```
solver/                 cargo workspace
  crates/world/         world file reader: blocks, boxes, movement effects, unknown space
  crates/port/          mechanical port of MC 26.2 movement, for fuzzing and verify
  crates/sim/           faithful and generic kernels, player state
  crates/view/          renders the world and annotations to images; knows only `world`
  crates/etas/          the toolkit, and the `etas` CLI for the tools that need no course code
  courses/<name>/       one crate per course: its phases as binaries, its runs/
  oracle/               Fabric client gametest that produces Minecraft's own trajectories
```

A course phase is a binary run with file arguments, parallel with rayon inside, the same
locally or on the cluster: `cargo run --release -p academy --bin optimize`. Its usage is in
the binary's module doc.

Formats and details live with their owners:

| what | where |
| --- | --- |
| plans | `PLAN-FORMAT.md` |
| trajectories | `crates/etas/src/trajectory.rs` |
| course files | `tools/fit_course.py` docstring |
| world files | `tools/extract_world.py` docstring |
| `.tas` tapes | `tools/extract_run.py` docstring |
| EMC's rules, as measured | `../docs/emc-course-rules.md` |
| ebounces | `../docs/ebounces.md` |
| other game behavior | `../docs/things-that-bite.md` |

Python stays where the work is empirical: it reads replays and fits course boundaries, and
it writes the world and course files that Rust reads. Pictures of runs in the world are Rust
(`etas view`, `etas::draw`), since course programs want them inline; charts of values over
ticks are Python, off artifact files.

## Physics

Written several times, on purpose. The implementations don't share code (the faithful
kernel takes only `port::mth`) and only `port` mirrors Minecraft's structure. Details and
measured gaps are in `crates/port/README.md` and `crates/sim/README.md`.

- **`port`** answers one question, "is this the Java", so nothing in it is clever or
  reorganized. A solver never needs it; `verify` does.
- **The faithful kernel** is the game's arithmetic, casts and sine table, with our own state
  and API, for solvers that need the exact game: finishing passes, a DP that flies exact
  steps. It has vanilla collision and no ablations. why? it is the game.
- **The generic kernel** is plain math over a float trait, with no casts or sine table, and
  most solvers use it. On dual numbers it gives gradients and Hessians, which the table would
  make zero almost everywhere. `f64` is more accurate than the game, not a copy of it, and
  `f32` is fast and approximate, not faithful (Minecraft mixes float and double). Its gap from
  the game is bounded on a recorded run. why? so a sign error fails a test instead of showing
  up as a slightly worse number.
- **A specialized kernel** (none yet) states a tolerance against the kernel it approximates,
  and tests it.

Turning a generic solution into one that holds under the faithful kernel is the course
program's job, not something the toolkit enforces.

## Rules for tools

- **`verify` is the only gate.** Nothing goes to the mod without passing it. why? the judge
  should be the least clever code we have, and a solver's own score is computed under
  whatever kernel, collision model and objective that solver chose.
- **Objectives are code, not data.** The library provides terms and each phase writes its own
  objective in Rust. why? a segment's objective is often ad hoc for that phase of that map,
  and no schema anticipates it. The cost is that an artifact can't be re-checked for
  optimality from the file, so `verify` checks only physical claims.
- **Extract a shared interface once two course phases actually compose, not before.** why?
  shooting, DDP and a tunnel DP need different state and terminal values, and a premature
  common interface becomes an adapter layer.
- **Add artifact types carefully.** A new type is a new thing every later tool may have to
  read.
- **Each kind of server event gets its own handling.** A booster's payload depends on the
  uplink, damage on the server's simulation, an imperfect bounce on neither, so they don't
  share one shape.

## Testing

The standard is faithfulness to Minecraft, not bit identity: Minecraft isn't bit-identical
to itself (`Math.cos` can differ in the last place between JVMs and platforms), so a mismatch
at that level is noise and anything larger is a bug.

Each layer is checked against the one below it, never against elytrasim: `oracle` and real
play against the game, `port` against those, the faithful kernel against `port`, and the
others against their stated reference.
