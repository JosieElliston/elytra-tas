# Playback (`/etas play`)

`/etas play PLAN [runs=N] [settle=T] [maxfail=K]` flies `<profile>/elytra-tas/plans/PLAN.tas`
(solver/PLAN-FORMAT.md version 2 or 1, or a plain `.tas` tape) in a loop: reset
with the slot-1 item, wait for the exact start pose (and the glide flag off), `settle`
still ticks (default 10), then line 1; again after "Course complete!" plus 20 ticks or an
abort (leaving the plan after "Course complete!" counts as done, not as an abort), until N runs are done (default: until stopped) or K attempts in a row failed
(default 3). `/etas stop` stops and hands the keyboard back; `/etas status` shows the
line, segment, divergences, reactions and the last attempts with the segments they flew.
While it plays, keys and mouse don't move you. The logger records it as `P` records
(netlog.py). Mod 0.5.3; 0.4.0 flies version 1 only. Tests: solver/oracle/README.md,
"Playback tests".

- **A version-2 plan branches on what the server sends.** Before each line, the
  glide-offs and velocities (`set_entity_motion` for you; the last one counts) the client
  handled since the last tick are matched against the current segment's `@branch` lines
  for that tick, and a match switches to the child segment; segments can rejoin
  (`@goto`). An observation nothing takes (no branch, `@react` or `@ignore`) aborts the
  attempt ("unlisted": a wall-hit echo, a booster the plan didn't expect), and so does a
  segment ending where a branch waited for an event that never came ("missing"). After
  "Course complete!" an unlisted one ends the attempt as a success instead. Version 1 and
  plain tapes keep version 1's behavior: nothing aborts on an observation.
- **`hold_glide=1` is a singleplayer test hack, never for a real run, singleplayer TASes included.**
  During each attempt the integrated server doesn't end the player's glide, which is what `port`
  models, so a plan's main line flies with no glide-off to react to. why? singleplayer's
  glide-off can come too early for a perfect bounce, and this tests the rest of the plan
  anyway. It's nonvanilla movement on the server, so `/etas play` refuses it anywhere but an
  unpublished singleplayer world, and the log's `start` record and the chat mark such runs.
- **It allows `deftenvy`, plus `lqmbda` in singleplayer.** The plan's `account=` (if any) must match
  (singleplayer also lets `lqmbda` play a plan for `deftenvy`),
  and the server is whitelisted for the logger's own packets; singleplayer always counts.
  It also refuses what PLAN-FORMAT.md's "Checks" lists (an `@` line its version doesn't
  have, another `format=`, plan keys or directives without `format=`, a `ticks=` that
  doesn't count every tick line, a `press_jump` window whose tick before holds `J`, a
  branch target that doesn't start at the branch's tick, `set_velocity` branches without
  `velocity_tol=`, cycles, unreachable segments).
- **Every nonzero pose difference is logged, and one over `diverge_tol` aborts:** checked
  before each tick's physics against the previous line, keys released on that tick.
- **A glide-off or velocity is caught where the client handles it**
  (`handleSetEntityData` / `handleSetEntityMotion`, at the start of a frame, before its
  ticks), so it counts as observed before the next line, and `press_jump` goes in at the next
  `START_LEVEL_TICK`, the same input path as the tape's keys, so it lands in the physics
  of the first tick to see the flag cleared. Not earlier: that tick's own keys would
  overwrite it. A jump pressed by a reaction counts as held on the next line.
- **The camera is set each frame at `Minecraft.renderFrame`'s head**, after the mouse and
  before camera setup, so the lerp needs no frame-late prediction (unlike `tas_play`'s
  render events). The exact rotation goes back at `START_CLIENT_TICK`, before anything
  in the tick (the logger's `T` record included) reads it.

## The queue (`/etas queue on`)

For a program choosing plans as it goes, such as probes of a course's regions. `/etas queue on`
in game, then the program drops plans in `<profile>/elytra-tas/queue/`; each flies one
attempt, oldest name first. It stays on until `/etas queue off`, `/etas stop` or a disconnect.

- **Write a plan elsewhere and rename it in**, as anything ending `.tas` is taken at once.
- **Each plan's answer is `queue/done/NAME.json`**, written after the plan moves to
  `queue/done/NAME.tas`: `ok`, `reason`, `abort_kind`, `lines`, `divergences`, `max_dev`,
  `timer_line`, `finish_line`, `stop_reason` and `log`, the logger's file, which has the
  poses and packets. A plan that ends without "Course complete!" gets `ok: false` and the
  queue goes on. why? most probes never finish.
- **`queue/state` says `on` or `off <reason>`.** Anything but an attempt ending turns the queue off, leaving later plans
  unflown: a refused plan (its answer has `refused`), `/etas stop`, a hub kick, a death, a reset
  item that doesn't reset. why? those mean nobody should be flying blind.
- It has `/etas play`'s checks, account included, since it starts plans the same way.
