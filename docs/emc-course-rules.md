# EMC course rules (measured)

From the logger, 2026-10-20 01:41–02:06 (EMC, ~80–110 ms RTT). The server acts once
per tick, and its tick phase against the client's holds within ~5 ms over a 25-minute
session (it wanders up to ~30 ms over hours: overnight log of 2026-10-20 02:50–08:50), so a
response to a packet arrives in one of two delay bands one tick apart (about 120 and
170 ms after the send here). Fit regions with that in mind: a fixed-width delay window
gives false contradictions.

- **An unfocused client runs ticks in pairs.** At the 10 fps background/inactivity
  limit, two ticks run in each frame about 1 ms apart, then nothing for ~100 ms, so the
  moves go out in pairs too. Server timing results from such a run don't carry over to
  focused play. why? the overnight 2026-10-20 log ran like this after its first minute.

- **Boosters set your velocity to `speed × look` of one move you sent, bit for bit.**
  The look is Bukkit's `getDirection` (double trig of the float rotation, yaw wrapped);
  each axis is then clamped to ±3.9 and truncated to 1/8000 (the pre-1.21.9 velocity
  packet), and LpVec3-quantized (ViaVersion re-encoding it for 26.2):
  `etas::booster::payload`. All 206 booster velocities of the 2026-10-20 01:58 alpine
  log reproduce exactly, 5 of them a tick or two late (lag); `python3 tools/boosters.py scan`
  found such boosts on 14 maps (logs and replays up to 2026-11-02), speeds 1.4–9.5 b/t,
  and the clamp binds on classic urban slop's 5 b/t pads. The move is the one whose position was inside; the velocity acts on the
  physics of the tick `delay` after that move's tick, one server tick after the move's
  tick-end reply (which brings the booster sound on the first boost): 3 or 4 by session,
  the two bands above. Near a server tick edge it can switch within one pass (3 of 265
  alpine boosts). Ring frames are solid blocks.
- **Which ticks of a pass boost is not a function of your moves.** In a pass (moves
  inside the booster's box on consecutive ticks) the first boost comes within 3 ticks of
  entering, the 2 ticks after it boost or not (78 of 124 did, alpine), and every later
  tick inside does (all but 1 of 142) until you leave; no tick outside does. why it
  matters: a plan can't know the 2 maybe ticks ahead, so it branches on them (the payload
  of each is known). Standing in a booster doesn't follow this: it boosted every 10–25
  ticks. Standing on the second alpine booster's gold blocks without gliding did not
  kill you for up to 8.7 s, so the kill rule seems to exempt booster volumes (at least
  one other booster does the same).
- **Hard wall hits overwrite your velocity, ~3 ticks late (vanilla).** The server runs
  its own copy of your elytra physics; when it loses > 0.3 b/t of horizontal speed in a
  collision, `LivingEntity.handleFallFlyingCollisions` deals fly-into-wall damage,
  `markHurt()` follows, and `ServerEntity` sends the server's velocity to you as if it
  were knockback. The client applies it on arrival, so with ping it replaces the
  velocity of a later tick. Alpine log: 24 of 36 such hits echoed (the misses fit the
  10-tick damage cooldown), 4 of 21 smaller ones. A physics replay has to apply it.
- **A plan can hit a wall only on the server.** The server's copy moves from the pose you
  just sent by its own velocity, about `p_t + v_t`, where you fly to `p_t + v_(t+1)`, so in a
  turn past a corner its box can hit what yours clears. A smaller loss is silent: that axis of
  its velocity zeroes and stays wrong, so later hits vary by attempt. Creative takes no damage
  and gets no echo, so singleplayer won't show it unless you read the `X` records. Academy's
  aborts of 2026-11-05 02:39 were this, at plan ticks 267, 584 and 617. `Opts::server` keeps
  that box clear, and `p_t + v_(t-1)`, a copy a tick behind, which is where all 8 aborts of 25
  flown on 2026-11-05 03:14 hit. `tas` prints any hit left and fails on it.
- **Timer** is wall-clock time between the "Timer started!" and "Course complete!"
  moves, rounded to 0.02 s: five identical 647-tick runs gave 32.34 or 32.36
  depending on whether that interval was under or over 32.35 s.
- **The number in the chat is the true time, in whole ticks.** Its digits below a tick are
  noise: the timer is wall-clock, up to 0.03 s off a whole number of ticks (academy: 31.22 s
  for 625).
- **The timer starts on a move that changes your block while your previous position was outside
  the start area.** As of 2026-11-05 23:05 EST, academy, with faces y >= 199 and z >= 205: exact
  for 253 of 270 logged timer starts, and the other 17 all predict an earlier move (a late chat, not
  a wrong rule). "First pose outside" gets 0 of the 153 prompt ones. So a slow exit runs on outside
  the box until the next whole-number crossing on any axis: a fall at x, z fixed fires at y < 198,
  which reads as y >= 198 if you assume the first pose outside. The message comes straight from the
  move handler, one ping after that move (age within 3 ms of the ping on most, unless the move
  lands in tick work, see below; it doesn't wait for the tick-end flush), and repeats of one plan fire on the same move. Flown out of sample at 2026-11-05 23:30 EST:
  14 of 14 new passes fired on the move this picks, up to 6 ticks after the first pose outside
  (5 checked against poses predicted before flying, 9 against the poses they sent). `/etas queue`,
  `tools/probe_timer.py`. It holds at a spoofed ping too (14 of 14 at about 106 ms measured, /pingequalizer
  at 150).
- **The finish is the same rule the other way round: a move that changes your block while your
  previous position was inside.** Academy, 2026-11-05 23:55 EST, with z >= -100: 7 of 7 passes
  predicted before flying at normal ping, 5 of which "first pose at z >= -99" gets wrong, and 4 of 4
  straight re-flights. Slow passes bracket z_min to (-100.20, -99.75]. So a run at z -100.07 then
  -98.73 finishes a move later, and the 38.44 s run that passed -99.55 at y -56.67 without finishing
  was just in one block until z crossed -99. x and y faces aren't found: it fired at x -7.26 to
  -1.68 and y -55.3 to -53.4 on these flights. `lqmbda`'s chat lags, so judge its finishes by
  the action bar.
- **A time is the realtime between the two moves that fired, as the server handled them,
  rounded to 0.02 s.** 14 clean academy runs, 2026-11-06: shown minus the gap between sending
  those moves is within 13 ms (mean 1.6, sd 6.4, about what rounding gives), while shown minus ticks x
  50 ms reaches 40 ms. The replies come a median 1.8 ms past the ping (max 8.3), so at this tick phase
  they don't wait for a tick. Most of the difference from whole ticks is the client's send timing:
  sends happen in frames, so they move with frame rate, while the network hardly varied.
- **A move that lands in tick work has its timer chat held until the work ends, the way a
  main-thread reply is.** Same session plans, `deftenvy`, 2026-11-06 at about 62 ms: moves 4.6 ms
  before the tick-end flush got chats a median 6.1 ms late (27 flights, min 2.1), moves 21 ms before
  it got 1.9 ms (14 flights), and tab-completion probes jumped the same way over the last 9-10 ms of
  the tick. So chat lateness is the wait plus the usual ~1.5 ms. Handling on the main thread is the
  plain reading. **The start clock waits too:** the action bar's ms value dates the reading, and it
  moves 0.86 +- 0.12 ms per ms of wait over those 41 flights plus one (1 = waits fully, 0 = read on
  arrival). Finished runs agree: over 22 `academy_535` runs, shown minus send gap falls -1.06 +- 0.29
  ms per ms of start clock delay, including a 26.72 whose start clock read 24 ms late. The finish
  chat waits as well (8.0 ms late with moves 5-9 ms before the flush, 2.5 ms outside the band), but
  the finish clock is open: over 33 runs the fit per ms of finish wait gives -0.15 +- 0.44 with the
  band at 9.5 ms and 0.39 +- 0.39 at 11 ms, where waiting is +1 and arrival is 0. Rounding to 0.02 s
  is most of that error, and only 2 of those finishes clearly missed the band. To stay out of the
  question, keep moves well before the band, and mind the drift (see tick-phase.md). `tools/tick_wait.py`
  places chats, probes and the clock in the tick, and its `--ping` shows whether the ping moved, as
  the phase can't be read while it does.
- **With moves at the flush, the send gap picks a run's rounding.** 14 of 14 `academy_535` runs
  with mod 0.5.4, 2026-11-06 01:35-01:42 EST, moves within about 5 ms of the flush: 535 ticks, send
  gap 26746.8-26755.1 ms, shown 26.74 for gaps up to 26748.4 and 26.76 from 26748.7. Send gap minus
  ticks x 50 ms has sd 2.6, against 13 under the AFK frame limit, so that tightening is mostly the
  patch and not the phase. Deeper in tick work the send gap stops picking it (5 of 8 runs at
  01:50-01:57, ping spikes in that batch), as a late start clock moves the time.
- **`academy_535` aborts have at least two causes, so don't blame ping alone.** Ping spikes fit one
  batch: 6 of 14 flights left the plan by 0.07-1.4 blocks at scattered lines, each with a spike of 162 ms
  or more in its last 8 s (2026-11-06 01:50 EST). But at 02:00, 3 of 16 left it at line 611 by the
  same 1.51 blocks with ping at most 102 ms, and 1 at line 390 with a 218 ms spike. Lines 610 and 618
  aborted by 1.2-1.5 blocks before mod 0.5.4 too. Cause of the line 611 group not examined.
- **Timer chat lateness differs by session or account, and phase doesn't explain it.** In 19 logs
  on `lqmbda` (2026-10-31 to 11-04, hand flown) the median chat is 15-24 ms late at every phase,
  against 1.4-10.2 ms in 7 logs on `deftenvy` (11-04 to 11-05), one of them between `lqmbda` logs on
  the same mod. But on 2026-10-20 `deftenvy` shifted about 10 ms within an hour, though those logs
  have no pings to rule out a ping change. Not settled; don't take a late chat as proof of tick work.
- **Minecraft's `inactivityFpsLimit: afk` changed that timing during playback before mod 0.5.4, when
  playback didn't count as input, so check it when reading older logs.** `FramerateLimitTracker` (26.2) caps at 30 fps 60 s after the last input and 10 fps after
  600 s, and input means a key, mouse button, scroll, or mouse move in the active window. Moves are
  48-53 ms apart uncapped, 33/67 ms at 30 fps, and 100 ms apart in pairs at 10 fps, when a reply
  can't say which of a pair it answers, so don't time rules at 10 fps. The logger records keys but
  not the mouse, so a log's time since the last key only bounds idle from above.
- **Timer and finish events reach you a constant delay after the pose they judged.** As long
  as the timer chat's delay: on academy, 2 ticks for 170 of 199 timer starts in the
  2026-11-03 20:52 log, 5 for 87 of 90 in the 1.21.11 replay. The action bar shows
  `Time: X   Best Time: Y` each tick and a blank line at the stop, which is on that delay.
  "Course complete!" can come later, by account, in every replay up to 2026-11-04: `deftenvy`
  0 ticks after the blank on 676 of 677 finishes; `lqmbda` 3-5 on most of 225; another
  player's client 3-5 on all 5. So `fit_course.py` uses the number from the chat and the blank
  line's tick, not the chat's tick.
- **Region faces are mostly whole numbers of the feet position, with no hitbox offset.** Refit
  2026-11-04 22:08 EST: academy y >= 199, z >= 205; burrowed grove x >= -6, finish x <= -35;
  downtown x < -27, 86 <= y < 99, z < -64; alpine z < -124, finish y <= 186, z >= 208. Offsetting
  by the 0.3 half-width doesn't fit: x >= -6 is bracketed to [-6.005, -5.955], x < -27 to
  [-27.149, -26.989]. Not all fit whole: downtown's finish z_max fits [-90.262, -90.152]. These fits assume the first
  pose over the face fires, which the two rules above replace, so they may shift. `course.json`'s
  academy finish still says z >= -99.7, a value that compensated for this; the real face is -100.
  why it matters: at normal exit speed one tick of travel is ~1.5 blocks, so a wrong delay
  makes a wrong face look just as good. Academy's start also fits y >= 198.56, z >= 203.45 at
  a delay a tick shorter. It's rejected on the likelihood with +-1 tick per event (about 16
  nats worse) and on those faces not being round, but only three slow exits (under 0.9
  blocks/tick) test it directly, and they lean the other way. Slow passes with `deftenvy`
  would settle it.
- **Start area** is box-like in bulk (canyon x < −266, 168 ≤ y < 177–178, z ≥ 164;
  super classic downtown −52 ≤ x < −27, 86 ≤ y < 99, z < −64) but not one box: near
  canyon's z ≈ 195–199 corner the −x face sits near −265.2, and landing at y = 167
  started the timer after 300 ms airborne below 168 did not. Leaving it without gliding
  starts the timer and kills you in the same tick.
- **Canyon finish** is no single box, sphere or cylinder under any delay; probably a
  union of boxes.
