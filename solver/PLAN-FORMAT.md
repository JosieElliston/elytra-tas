# Plan format, version 2

What the `/etas` mod plays. Written by the solver (`crates/etas`), read by the mod in
Java. This file is the spec; both sides follow it, and a change here bumps the version.

A plan is a `.tas` tape (format in `extract_run.py`'s docstring) with extra header keys
and directive lines, optionally split into segments that playback switches between on
what the server sends. A plain `.tas` tape is a valid plan with no reactions, and a
version-1 plan (`format=etas-plan 1`: one segment, `@react` only) is still valid and
flies as before. why? `tas_play` and `extract_run.py` already speak `.tas`, so a plan
stays inspectable and flyable with the old tools; the writer writes version 1 whenever
the plan has no branches, so mod 0.4.0 still flies those.

## Header (`key=value` lines before the first `@segment`)

- `format=etas-plan 2` (or `1`): required for anything a plain tape doesn't have.
- `account=deftenvy`: playback refuses to start unless the active account matches.
  why? AGENTS.md: anything that moves the player runs only on the alt.
- `course=<name>`, `course_sha256=…`, `world_sha256=…`: what it was verified against.
- `diverge_tol=<blocks>` (default `1e-4`): see Playback.
- `velocity_tol=<blocks/tick>`: how far an observed velocity may be from a branch's
  payload, per component, and still match it. Required when a plan has a
  `set_velocity` branch. why stated, not fixed? what is exact depends on the server
  and the wire: EMC sends velocity in 26.2's low-precision vector encoding, and a
  booster model may predict it only so well.
- the tape's own keys (`start=`, `ticks=`, `source=`, …) as `.tas` defines them. `start=`
  is the start contract: playback resets with the slot-1 checkpoint item, as `tas_play`
  does, and line 1 is the first tick after the pose is at `start` exactly. `ticks=`
  counts every tick line in the file, all segments.

## Tick lines

As `.tas`: `tick yaw pitch keys x y z`. `yaw` and `pitch` are float bits in hex, set
with `setYRot`/`setXRot` before that tick's physics. `keys` is a subset of `WASDJR`
(`J` jump, `R` sprint) or `-`. `x y z` is the expected pose after the tick, as double
bits. The expected poses assume every reaction in the segment happens as planned.

## Segments

The lines before the first `@segment` are the root segment (logs call it `root`): its
tick lines are ticks 1, 2, … in order. `@segment <name> <start>` begins a named segment,
which runs to the next `@segment` or the end of the file; its tick lines are ticks
`start`, `start + 1`, … in order, and may be none. Names are `[A-Za-z0-9_.+-]+`, unique,
not `root`. Directives belong to the segment they are in. Ticks are the plan's own
numbering throughout, so a segment's tick `t` is the `t`-th tick after the start, on any
path that reaches it.

A segment's *end* is the tick after its last line (`start` for an empty one).

## Directives

Events: `glide_off` (the server's entity data clearing the local player's fall-flying
flag) and `set_velocity` (`set_entity_motion` for the local player: boosters, the
wall-hit echo). An event is *observed before line t* when the client handled it after
line `t-1`'s physics and before line `t`'s; for `set_velocity` the observation is the
last payload handled in that gap (an earlier one was overwritten before any physics saw
it).

- `@react <t0> <t1> <event> <action>`: during ticks `t0..t1` inclusive, on the tick on
  which `event` is first observed, do `action` in that tick before its physics. At most
  one firing per directive. If `event` doesn't come in the window, nothing happens, and
  the expected poses say whether that was fine.
  - events: `glide_off`. actions: `press_jump` (jump down for that tick; the tape's
    keys for the tick before must not hold `J`, so it is an edge, and
    `tryToStartFallFlying` restarts the glide).
  - the window lies in its segment, and so does the tick before it: `start < t0`
    (`1 <= t0` in the root, whose tick 0 is the start) and `t1 < end`.
- `@branch <t> glide_off <child>` and `@branch <t> set_velocity <vx> <vy> <vz> <child>`
  (`v` as double bits, as the client applies it): if the event is observed before line
  `t` (a velocity within `velocity_tol` of `v` on every axis), playback goes on in
  `child`, whose `start` is `t`. `start <= t <= end` of the branch's segment. Children
  may have several parents: the plan is a DAG, so paths whose states meet again share
  what follows.
- `@goto <child>`: after the segment's last line, if no branch took the observation
  before `end`, go on in `child`, whose `start` is this segment's `end`. At most one.
- `@ignore <t0> <t1> <event>`: an observation of `event` before a line in `t0..t1`
  (`start <= t0 <= t1 <= end`) that no branch or reaction takes is logged and doesn't
  abort. Whether it was harmless, the expected poses say.

why per-tick branches, not a window of cases? the arrival tick is part of the
observation (a velocity at tick 41 and the same one at 42 fly differently), so each
tick's outcomes are their own children anyway, and the parent's lines between them are
what "nothing yet" flies.

The perfect-ebounce reaction: pressing jump on the tick the glide-off arrives flies like
a bounce that never lost the glide, so the solver models a perfect bounce and the plan
says how to make it one. "Like" has conditions, which `etas verify` checks per window
tick by replaying the reacted branch, and the writer only emits ticks that pass:

- the player must be airborne on that tick (a glide-off while still on the ground can't
  be answered: that is a `@branch … glide_off` to what the lost glide flies), and jump
  must not be held the tick before (no edge), so the tick after the bounce jump is
  usually out;
- `canStartSprinting` runs before the glide restarts, so with W and sprint held and
  sprint off, the reaction tick starts a sprint;
- the glide-off carries the server's standing pose, so that tick moves a 1.8-tall box
  (matters only under a ceiling within 1.2 blocks).

The writer puts each directive on the line before the tick line it names first, and a
segment's `@goto`, and its branches and ignores at `end`, after its last tick line. A
reader accepts directives anywhere in their segment. A reader that meets an `@` line its
version doesn't have refuses the plan (version 1: `@react` only); so does `@server`,
which only recorded tapes carry.

## Example

Tick lines shortened to `t …`; velocities shown as decimals, written as bits.

```
format=etas-plan 2
account=deftenvy
velocity_tol=1e-6
start=…
ticks=…
1 …                                  # root: ticks 1-40
…
@branch 40 set_velocity 0.0 1.4 0.7 boost   # the booster, at its early delay
@branch 41 set_velocity 0.0 1.4 0.7 late    # or a tick later
40 …                                 # root's 40: nothing before it yet
                                     # root ends at 40: nothing before 41 either aborts
@segment boost 40
40 …
…
@goto after
@segment late 41
41 …
…
@branch 61 glide_off lost            # a bounce the glide-off finds still on the ground
@react 62 66 glide_off press_jump    # or a perfect one
61 …
…
@goto after                          # late runs to 69
@segment lost 61
61 …                                 # walk, jump, redeploy; ends where the plan ends
@segment after 70
70 …
```

## Checks

A reader refuses, besides malformed lines: a `ticks=` that doesn't match; a segment's
tick lines out of order; a header line after the first `@segment`; a branch or goto to a
missing segment or one that doesn't start where it says; a cycle; a segment the root
can't reach; two branches of a segment at one tick that one observation could both
match (`glide_off` twice, or velocities within `2 velocity_tol` on every axis); a
`glide_off` branch inside a `glide_off` reaction's window of the same segment; a branch
that can never match because entering its segment at that tick took the same kind of
observation; and, for `press_jump`, a window whose tick before holds `J`.

## Paths

A path is what one sequence of observations flies: from the root, at each segment
either a branch at its tick (the segment's lines before it, then the child) or none
(all its lines, then the goto). It ends at a segment with no goto: at the plan's end,
or, if that segment has a branch at its end, at an abort (the event it waits for never
came). Each path is a tape (ticks 1 onward, no gaps) plus the arrivals it assumes (each
branch taken, at its tick), plus its segments' reactions with each window cut at the
tick the path leaves that segment. `etas::plan::Plan::paths` lists them and
`verify_paths` verifies every one that reaches the plan's end; nothing goes to the mod
unless they all pass.

## Playback

Before each line `t` (after line `t-1`'s physics):

1. Compare the player's pose with line `t-1`'s expected pose, from the segment that
   flew it. Log every nonzero difference. Abort (release all keys, stop, report the
   tick and the difference) once it exceeds `diverge_tol`. why a tolerance, not exact?
   the game's `Math.cos` differs from the solver's in the last place at about 0.5% of
   pitches (`crates/port/README.md`), so 1-ulp differences are expected and not a sign
   of desync. Once "Course complete!" has come, leaving the plan ends the attempt as a
   success instead (the run is timed; a landing or wall echo after it isn't the plan's).
2. Take the observations before `t`. While the current segment has a branch at `t` that
   an untaken observation matches, take it and go to the child; else, if `t` is the
   segment's end and it has a goto, go there; repeat.
3. Each untaken observation: a `glide_off` in an unfired reaction's window fires it;
   one in an `@ignore` window is logged; any other aborts the attempt ("unlisted"),
   unless "Course complete!" already came: then the attempt ends as a success (the run
   is timed, and what the server does after it isn't the plan's). In a version-1 plan
   or a plain tape nothing aborts here: observations without a reaction are logged, as
   version 1 did.
4. If `t` is the segment's end: if it has a branch at `t`, abort ("missing": the
   expected event never came); otherwise the plan is over, and the attempt is a success
   if "Course complete!" came.
5. Set line `t`'s rotation and keys (and a fired reaction's jump).

A reaction that fires outside its window, or an event the plan has no reaction for at a
tick where the expected poses need one, shows up as divergence.
