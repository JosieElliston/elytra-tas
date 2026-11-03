# oracle

Minecraft's own trajectories for fuzzing `port` (ARCHITECTURE.md, "Testing the
physics"): a Fabric client gametest that builds random block setups in a singleplayer
dev world, drives the player for a few hundred ticks the way `tas_play` does, and
writes each case to `cases/<kind>-<seed>/`. `crates/port/tests/oracle.rs` replays them.

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

## Things that will bite

- **The gametest dependencies go after `fabricApi { configureTests }`** in
  `build.gradle`: that block creates the `gametest*` configurations.
- **Don't use `waitForChunksRender()` after a far teleport**: at render distance 12 it
  timed out (10 s default). The test waits until the client has every placed block as
  the server has it instead.
