# Plan format, version 1

What the `/etas` mod plays. Written by the solver (`crates/etas`), read by the mod in
Java. This file is the spec; both sides follow it, and a change here bumps the version.

A version-1 plan is a `.tas` tape (format in `extract_run.py`'s docstring) with extra
header keys and extra directive lines. A plain `.tas` tape is a valid plan with no
reactions. why? `tas_play` and `extract_run.py` already speak `.tas`, so a plan stays
inspectable and flyable with the old tools.

## Header (`key=value` lines before the first tick line)

- `format=etas-plan 1`: required for anything a plain tape doesn't have.
- `account=deftenvy`: playback refuses to start unless the active account matches.
  why? AGENTS.md: anything that moves the player runs only on the alt.
- `course=<name>`, `course_sha256=…`, `world_sha256=…`: what it was verified against.
- `diverge_tol=<blocks>` (default `1e-4`): see Playback.
- the tape's own keys (`start=`, `ticks=`, `source=`, …) as `.tas` defines them. `start=`
  is the start contract: playback resets with the slot-1 checkpoint item, as `tas_play`
  does, and line 1 is the first tick after the pose is at `start` exactly.

## Tick lines

As `.tas`: `tick yaw pitch keys x y z`. `yaw` and `pitch` are float bits in hex, set
with `setYRot`/`setXRot` before that tick's physics. `keys` is a subset of `WASDJR`
(`J` jump, `R` sprint) or `-`. `x y z` is the expected pose after the tick, as double
bits. The expected poses assume every reaction below happens as planned.

## Directive lines

`@react <t0> <t1> <event> <action>`: during ticks `t0..t1` inclusive, on the tick on
which the client first handles `event`, do `action` in that tick before its physics.
At most one firing per directive. If `event` doesn't arrive in the window, nothing
happens, and the expected poses will say whether that was fine.

- events: `glide_off` (the server's entity data clearing the local player's
  fall-flying flag).
- actions: `press_jump` (jump down for that tick; the tape's keys for the tick before
  must not hold `J`, so it is an edge, and `tryToStartFallFlying` restarts the glide).

why? a perfect ebounce: pressing jump on the tick the glide-off arrives flies like a
bounce that never lost the glide, so the solver models a perfect bounce and the plan
says how to make it one. "Like" has conditions, which `etas verify` checks per window
tick by replaying the reacted branch, and the writer only emits ticks that pass:

- the player must be airborne on that tick (a glide-off while still on the ground can't
  be answered), and jump must not be held the tick before (no edge), so the tick after
  the bounce jump is usually out;
- `canStartSprinting` runs before the glide restarts, so with W and sprint held and
  sprint off, the reaction tick starts a sprint;
- the glide-off carries the server's standing pose, so that tick moves a 1.8-tall box
  (matters only under a ceiling within 1.2 blocks).

The writer puts each `@react` line just before its `t0` tick line; a reader accepts
them anywhere after the header.

Reserved, not in version 1: `@segment`, `@branch` (plans that branch on an observed
packet: ARCHITECTURE.md "Artifacts"). A reader that meets an `@` line it doesn't know
refuses the plan.

## Playback

- Before each tick's physics, compare the player's pose with the previous line's
  expected pose. Log every nonzero difference. Abort (release all keys, stop, report the
  tick and the difference) once it exceeds `diverge_tol`. why a tolerance, not exact?
  the game's `Math.cos` differs from the solver's in the last place at about 0.5% of
  pitches (`crates/port/README.md`), so 1-ulp differences are expected and not a sign
  of desync.
- A reaction that fires outside its window, or an event a plan has no reaction for at a
  tick where the expected poses need one, shows up as divergence.
