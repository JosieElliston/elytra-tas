# oracle

Minecraft's own trajectories for fuzzing `port`: a Fabric client gametest that builds random block setups in a singleplayer
dev world, drives the player for a few hundred ticks the way `tas_play` does, and
writes each case to `cases/<kind>-<seed>/`. `crates/port/tests/oracle.rs` replays them.

why a client gametest, not a server one? a server mock player skips `LocalPlayer.aiStep`
(jump edges, glide start), takes wall damage, and loses the glide on the first ground
tick, so it can never produce the states ebounces and glide starts depend on.

```
cd ../../mod && ./gradlew build                # the logger jar this needs
cd ../solver/oracle && ./gradlew runClientGameTest
cd .. && CARGO_TARGET_DIR=target/agent-port cargo test -p port --release --test oracle -- --nocapture
```

`runClientGameTest` opens a game window with an offline dev account and never
connects to a server. Ticks run at 20 per second, so a case takes its length in game
time plus a second or two of setup: 14-22 s by kind, about 9 min for the default 32
cases and 12 MB of files (2026-11-03 01:10 EST). It clears `build/run/clientGameTest`
at each start, not `cases/`; a case it writes replaces the one of the same name.
Which cases:

- `ORACLE_CASES=walls:3,air:12`: those, or
- `ORACLE_KINDS=air,walls` (default all four) times `ORACLE_SEEDS=1-8` (the default).

A case is a function of its kind and seed alone, its coordinates included, so the same
seed rebuilds the same world and inputs. why the coordinates? the float bits of the
physics depend on them. Rerunning three cases in another order gave identical `T`
records and sidecars (2026-11-03 01:20 EST); that holds on the same JVM (below), the
same Minecraft, Fabric API and logger, and the same generator code.

## Kinds

Every case starts standing still, on its floor or on a glass block removed right
before the first tick, and deploys with a jump press in air. Rotations are random in
segments: held, drifting, fast yaw spins, random each tick, extreme or out-of-range
pitch (for `setXRot`'s clamp), jitter. Yaw is never wrapped.

- `air`: 300 ticks of gliding in open air with random keys; pulls up below y 110.
- `walls`: a wall, an inside corner, a hollow room with pillars, or a ceiling, ahead
  of a diving approach; then random input, down to a catch floor.
- `ground`: 400 ticks of walking, sprinting, strafing, sneaking and jumps on a floor
  with step-ups (slab rows, stairs, snow layers, carpet, paths), ice and soul sand, a
  low roof to bump into, a raised block with a slab-and-stair way up, and a drop to a
  lower floor; 40% of airborne spells deploy.
- `landing`: a dive onto a floor 3-30 blocks down with random things on it, then
  ebounces and walking.

Surfaces mix full blocks with odd shapes (fences, walls, panes, bars, trapdoors,
anvils, hoppers, cauldrons, lecterns, chains, ...). Nothing climbable, fluid, bouncy or
sticky, and nothing that changes state on its own. why? port has those as
`Unsupported`, and a block that changes mid-case (a daylight detector's power, a
copper chain oxidizing) makes the world file wrong for part of it. Random ticks are
off for the same reason: with them on, 9 placed blocks of `ground-3` changed.

## Case files

`log.tsv.gz` is the elytra-tas logger's log of exactly the case's ticks (format in
`netlog.py`; its `T` records are tick 1 onward). `oracle.tsv`, tab-separated:

- `M key=value...`: kind, seed, steps, stop reason, origin, structure (walls),
  `blocks_changed`, and the Minecraft, JVM and OS versions.
- `A x0 y0 z0 x1 y1 z1`: a box, inclusive, that is air apart from the `B` blocks:
  the path's and the placements' bounds, 8 blocks wider. Outside it is unknown.
- `B x y z state`: every non-air block in the box, read back after the case.
- `C x y z start end`: a placed block that changed during the case (none expected:
  random ticks are off).
- `P tick yaw pitch held cos STATE`: at the head of `LocalPlayer.tick`, after the
  driver set the input. `yaw pitch` are what was passed to `setYRot`/`setXRot`,
  `held` the keys held (logger input bits; a jump press is sent as `autoJumpTime = 1`
  with jump off, so `STATE`'s keys lack it), `cos` the JVM's `Math.cos` of the lean
  angle.
- `Q tick STATE`: at its return.

`STATE` is what port reads and the `T` record lacks: pose, onGround,
horizontalCollision, verticalCollision, verticalCollisionBelow,
minorHorizontalCollision, mainSupportingBlockPos (`x,y,z` or `-`), onGroundNoBlocks,
fallDistance, fallFlying, sprinting, the sprint speed modifier, noJumpDelay, jumping,
xxa, yya, zza, `input.keyPresses` (bits), `input.moveVector` x y, sprintTriggerTime,
autoJumpTime, crouching. Floats are written widened to double, as the logger does.
`tick` is the logger's tick number.

## What it doesn't cover

- **Double-tap sprint, at any sprint window.** The driver sets the keys before `aiStep`
  samples last tick's, as playback does, so a press never counts as new: in 18910 records
  of the default cases, recorded at window 7, `sprintTriggerTime` was always 0, and port
  matched them at both 7 and 0 (2026-11-04 21:58 EST). The test client runs at 0 anyway,
  as port does.
- **The JVM is this machine's, not the one the game is played on.** `Math.cos`, which
  the lift uses, may differ in the last place between JVMs and platforms, so a case
  from here is the game on this JVM. On aarch64, Temurin 25.0.5 (the gametest's) and
  Modrinth's Zulu 25.0.4.1 gave identical `Math.cos` on 6M random pitches, and both
  differ from fdlibm (`StrictMath.cos`, which `port::mth::java_cos` copies) at 0.55%
  of float pitches, even under `-Xint` (measured 2026-11-03 01:00 EST). `oracle.rs`
  counts a gliding tick that mismatches where the recorded `cos` differs from
  `java_cos` as that noise, not a failure.
- **No server effects but its flag updates.** The player is invulnerable, so no wall
  or fall damage and no velocity echo; no lag: the integrated server runs in lockstep
  with the client, so the glide-off always arrives the same number of ticks after a
  landing, and perfect ebounces never happen. The server also echoes the client's own
  sprint flag (`set_entity_data`) and sprinting speed modifier (`update_attributes`) a
  tick late, which briefly undoes a sprint start or stop. `oracle.rs` applies those
  (glide flag, sprint flag, pose, sprint modifier) where the log shows them and stops a
  continuous replay at any other change between ticks (a teleport, a correction).
- No fluids, climbables, bouncy or sticky blocks, offset shapes (bamboo), entity
  collisions, fireworks, boosters, effects or creative flight. The flat world's ground
  is below every case; the box assumes nothing else was ever built there.
- **Some contacts are rare.** In the default corpus (2026-11-03 01:20 EST): 6 ticks
  ending in a ceiling contact while airborne, 11 step-ups, 38 ticks of wall contact
  while gliding, against 208 ticks of horizontal collision in all, of 9455. The walls kind's
  ceiling and room setups mostly end on the floor.
- The rotation is set right before physics each tick, as in playback, so a mouse
  turn between ticks never reaches the physics; turning within a tick isn't covered.

## Playback tests

`PlaybackGameTest` runs after the oracle cases in the same client, in its own world,
and tests the mod's `/etas play` (`docs/playback.md` at the repo root). Its plans,
logs and the held case go to `build/playback/`. `PLAYBACK_TESTS=a,b,c,d,e` picks (default
all; `none` skips). The quick way to run only these:

```
ORACLE_CASES=air:1 ./gradlew runClientGameTest       # about 6 min in all (2026-11-03)
cd .. && ORACLE_CASES_DIR=$PWD/oracle/build/playback/cases CARGO_TARGET_DIR=target/agent-port \
    cargo test -p port --release --test oracle -- --nocapture
```

- account checks: `deftenvy` is allowed everywhere, `lqmbda` only in singleplayer,
  other accounts nowhere; valid plans start for both permitted singleplayer accounts,
  and `lqmbda` can also play plans marked `account=deftenvy` there.
- refusals: an unapproved account, and plans with `format=etas-plan 3`, `@branch` or
  `@segment` in version 1, an unknown event, `@react` in a plain tape, a mismatched `account=`, a
  wrong `ticks=`, no `start=`; in version 2 an unknown `@` line, `@server`, a
  `set_velocity` branch without `velocity_tol=`, a missing child, a child that starts
  elsewhere, an unreachable segment, two branches one velocity would match.
- (a) a plan made from the oracle case `PLAYBACK_CASE` (default `air-1`; it must be in
  `cases/`): its inputs, and the poses of its log's END records. Flown twice with
  `runs=2`: as of 2026-11-03 06:15 EST, 0 divergent ticks in both.
- (b) ebounces: off a 50-block pillar onto the flat world's ground, a hop after each
  landing. The Driver flies it once with the server's glide-off held (`GlideHoldMixin`),
  which is what port models: nothing ends the glide. That run is written as the oracle
  case `build/playback/cases/ebounce-held`, and port's replay above must be
  continuous-exact on it (240/240 ticks, 2026-11-03). Its inputs and poses become a plan
  with `@react L+1 L+6 glide_off press_jump` after each single-tick landing L, cut at the
  first longer contact. Playback flies it 9 times with the vanilla glide-off: 36 of 36
  reactions fired, 0 divergent ticks, and the glide-off was first seen before line L+3 in
  all 36 (2026-11-03 06:15 EST). The same plan without reactions aborts at the first
  landing's L+3, and flies clean with `hold_glide=1`.
- (c) the air case's plan with line 150's x off by 0.01, and the player moved 1e-3 in x
  on the tick line 120 is set in attempt 1 (`maxfail=2`): the aborts come after lines 120
  and 150, each tick's physics runs with no keys, and the input is handed back at the stop.
  The same nudge after "Course complete!" ends the attempt as a success.

- (e) the queue (`docs/playback.md`): a plan with no finish gets its answer and the queue goes
  on; a refused plan gets `refused`, turns it off and leaves later plans unflown.
- (d) branches (format 2), from Driver runs that get the same server events at the same
  lines as playback. The test hands them to the client through its own handlers
  (`handleSetEntityMotion`, and `handleSetEntityData` with the glide bit cleared) at
  `END_CLIENT_TICK`, so they count as observed before the next line, the way a real
  packet handled at the next frame's start does. `branches`: standing on the ground,
  up 0.6 or 0.4 before line 20 (children A, B) or 0.6 before 21 and 22 (C, then C2:
  a booster's re-send); all three rejoin J at 50, where every recording stands still
  with the same poses bit for bit. `glide-branches`: gliding a circle off an 80-block
  pillar, a glide-off before 35 or 36 (G, G2). As of 2026-11-03 10:56 EST all 13
  attempts flew the expected path: 0 divergent ticks on every exact path, 15 (max
  6.5e-6) for a velocity 5e-7 off the payload (`velocity_tol=1e-6`); an `@ignore`d
  echo of the player's own velocity went on, one outside the window aborted
  (unlisted), as did a velocity no branch names and a glide-off at 30; a C without
  its second velocity aborted at 22 (missing). `crates/etas/tests/plan.rs`
  (`gametest_plans_parse`) reads the two plans with the Rust reader and lists the same
  paths.
- **(d) fakes the server, so it tests the mod's logic, not EMC's timing or payloads.**
  A real `set_entity_motion` goes through the wire's low-precision vector encoding
  (singleplayer's in-memory connection skips it), and on EMC arrives in one of two
  delay bands; the server's flag still says gliding after a faked glide-off, which
  matters only if the player redeploys before the server ends its own glide.

The test runs as `deftenvy` by replacing `Minecraft.user` (`MinecraftAccessor`), and
stands in for EMC: a stick in slot 1 teleports to the start on the server, and the test
sends "Course complete!" 25 lines before a plan's end.

- **Singleplayer has a fixed glide-off delay and no lag**, so (b) shows the reaction path
  and the physics claim, not EMC's timing: there the glide-off comes 3-4 ticks after a
  landing (`docs/tick-phase.md` at the repo root), and a perfect bounce needs the player airborne by then.
- **A platform start needs the platform gone right before line 1.** The oracle removes
  it between ticks; the test removes it on the tick line 1 is set, from a
  `START_LEVEL_TICK` listener registered after the mod's, so after playback's handler and
  before physics; the server removes its copy right before it handles the next move
  (`ServerMoveMixin`). why there? queued with `execute`, the removal sometimes ran after
  the client's first move off the platform, and the server sent the player back (both
  attempts of (c), 2026-11-03 10:46 EST); waiting for it on the client thread can't
  work, since the integrated server runs in lockstep with the client.

## Things that will bite

- **The gametest dependencies go after `fabricApi { configureTests }`** in
  `build.gradle`: that block creates the `gametest*` configurations.
- **Don't use `waitForChunksRender()` after a far teleport**: at render distance 12 it
  timed out (10 s default). The test waits until the client has every placed block as
  the server has it instead.
