# elytra-tas

Replay a recorded course run on EMC from a Flashback recording, in a loop.

```
python3 extract_run.py REPLAY.zip --list                 # completed runs in a replay
python3 extract_run.py REPLAY.zip -o tas/NAME.tas        # last completed run -> tape
\tas_play NAME runs=1 trace                              # in game, on deftenvy
```

Flashback replays live in `<profile>/flashback/replays/`. `tas_play.pyj`,
`undeploy_test.pyj` and `tas/` are symlinked into `<profile>/minescript/`; logs go to `minescript/tas_logs/`.

- `flashback.py`: Flashback replay reader (chunk/action format, local player poses,
  chat/title text, shared flags, held slot)
- `players.py`: every player a replay saw, one `Life` per spawn; its docstring says how
  remote poses differ from local ones (`python3 players.py REPLAY.zip [--dimension D]`)
- `segments.py COURSE_DIR REPLAY... [--tapes]`: cuts every player's attempts out of those
  replays and times them against the course's tape, into `runs/segments.jsonl.gz` and
  `runs/splits.md` (start-area exits, best stretch between gates, who covered what); `--tapes`
  writes local finished runs to `runs/seeds/`. Remote players' stretches are rough; see its
  docstring.
- `extract_run.py`: finds runs, reconstructs per-tick inputs, writes a `.tas` tape and
  checks every gliding tick against elytra physics
- `physics.py`: MC 26.2 elytra physics, float-faithful, for that check (needs numpy)
- `world.py`: the blocks the client had at a replay tick (block updates applied,
  unknown space explicit, `min_y` from registry data). `extract_world.py REPLAY.zip
  --tick T [--bounds X0 Y0 Z0 X1 Y1 Z1] -o OUT.world.gz` writes them as a world file
  for `solver/` and prints a coverage report; its docstring is the format spec.
- `tools/block_dump.sh`: regenerates the block table `data/blocks-26.2.tsv.gz`
  (`tools/BlockDump.java` against the Loom-cached 26.2 jar). World files name the
  table by hash, so regenerating it means re-extracting them.
- `tas_play.pyj`: Minescript (Pyjinn) player; `/etas play` in `mod/` (below) replaces it
- `undeploy_test.pyj`: tests the [undeploy](../undeploy) mod on EMC: `/hub`, then
  ebounces in place under undeploy off and several N, and logs each landing's
  glide-off ticks and any glide lost in the air (`\undeploy_test`, on deftenvy)
- `pyjinn_check.sh FILE.pyj...`: compiles `.pyj` files with the Pyjinn inside the
  installed Minescript mod, as `\script` does. Run it after every edit. why? Pyjinn
  accepts less than Python (e.g. `S["a"], S["b"] = 1, 2` is rejected: only plain
  names in tuple targets), CPython's `ast` passes those, and in game you see only
  the first error per launch. It catches syntax only, not runtime errors.
- `solver/`: the toolkit for min-time plans, [ARCHITECTURE.md](solver/ARCHITECTURE.md).
  `etas verify FILE --course solver/courses/NAME` replays a trajectory or `.tas` plan
  with the port and the course's rules (exit 1 on a broken rule or a recorded state that
  doesn't reproduce); `etas tas2traj` / `traj2tas` convert. The trajectory format is in
  `crates/etas/src/trajectory.rs`. `cargo run --release -p burrowed_grove --bin
  recorded` is the smoke test of the chain on the recorded run; `--bin resolve [A..B ...]`
  then re-solves stretches of it with DDP on the generic kernel, re-flies them under the
  faithful kernel and verifies the spliced run (`crates/etas/src/solve/`).
  `cargo run --release -p super_classic_downtown --bin solve` makes a recorded run whose
  ebounces lost the glide into a branching plan (`runs/downtown.tas`): perfect bounces
  with `@react` windows, and a `@branch … glide_off` to a lost-glide flight wherever a
  glide-off can come at a tick no jump press rescues.
  `etas flight TAPE --course C -o OUT` cuts a recorded tape's flight into a trajectory
  starting in the air, from the deploy that left the start area; it needs no walking or
  key fits, so it works where a tape's pre-drop doesn't. `-p academy --bin flights` does it
  for all of `runs/seeds/`.
  `etas view RUN... --course solver/courses/NAME -o out.png [--ticks A B] [--collision] [--l-step]`
  renders runs in their world from above, the side and in perspective, with `verify`'s
  findings marked; a course program uses the `view` crate and `etas::draw` directly for
  anything else (`crates/view/src/lib.rs`). Textures come from the client jar
  (`$ETAS_MC_JAR`, then the loom and Modrinth copies); with none, flat-colored collision
  boxes, with a warning.
- `fit_course.py COURSE -o solver/courses/COURSE/course.json`: fits a course's timer
  start, finish and kill rules from the replays and logger logs its `COURSES` entry
  lists, each rule with the bracket its data allows; its docstring is the format spec.
  Faces the data can't reach are written as assumed and the rule as provisional.
- `mod/`: Fabric client mod, see below. `netlog.py`: reader for its logs; its
  docstring is the format spec (`python3 netlog.py LOG.tsv.gz --timeline`).

## Network/tick logger (`mod/`)

Logs what Flashback can't: exact client tick poses and keys, key presses and releases
between ticks, every outbound packet,
inbound packet decode and handle times, socket read/write times, and in singleplayer
the integrated server's view (move packets processed, player ticks, glide ends). One
gzipped TSV per connection in `<profile>/elytra-tas/logs/`. It starts on joining a
server or world (config via Mod Menu, YACL), or with an unbound toggle key. It only
observes, apart from the pings and probes of `/etas phase` and the playback of `/etas
play` below, so it may run on `lqmbda` too (playback refuses any account but `deftenvy`). Those go out only for whitelisted `user@server` pairs (Mod Menu →
Whitelist; `deftenvy@elytramc.net` by default, subdomains included, and singleplayer
always). why? they are packets a vanilla client wouldn't send. With the [undeploy](../undeploy) mod installed it
also records that mod's decisions as `U` records, since the packets it rewrites or makes
up don't show what the client saw.

```
cd mod && ./gradlew build
cp build/libs/elytra-tas-0.5.0.jar "$HOME/Library/Application Support/ModrinthApp/profiles/main/mods/"
```

Rename the previous version's jar in `mods/` to `.disabled`, as with emc-replay, and
restart Minecraft.

- **"On the wire" means the end of Netty's pipeline, not the network.** `W` is when a
  packet's final frame (compressed, encrypted) reaches the handler next to the socket;
  the `F` flush around it ends when the socket write returned, i.e. the bytes are in
  the kernel's send buffer. `R` is Netty's socket read. Kernel and network delay are
  not visible.
- **Mixins use `defaultRequire: 1`, so a broken injection crashes at launch.** why? a
  missing hook would otherwise give a log with silent holes. To check injections
  without launching the client, copy `mod/` to a scratch dir, move the mixin list from
  `client` to `mixins`, set `environment` to `*`, add a `main` entrypoint that calls
  `MixinEnvironment.getCurrentEnvironment().audit()` and halts, and `./gradlew
  runServer` (with `run/eula.txt`). `-Dmixin.debug.export=true` writes the mixed
  classes to `run/.mixin.out/`. This worked for 0.1.0 (2026-10-20 01:19 EDT).

## Tick phase (`/etas phase`)

![Server and client ticks, and the six intervals of a move's loop](docs/ticks.svg)

A client command in the same mod, on whether the logger runs or not. It splits the loop
from a client tick through a move, the server's reply and the next client tick that sees
it, in the order it happens, into net up + down (the round trip; the client can't split
it), server wait, server work (MSPT), client wait and client work, which add up to a
whole number of ticks. A second section has what `set_health` alone gives: server wait +
work, and the tick interval. `/etas phase help` explains each row. The figures come from
`python3 docs/figures.py`.

- **Server wait and server work come from probes.** The mod sends a tab-completion
  request 4 times a second at random times, for `/msg `, `/tell `, `/help ` or `/` in
  turn until one gets replies, then only that one. why not something with no
  suggestions? EMC sent no reply to any of 1113 requests for `/etas` (2026-10-29
  16:02–16:08 logs), though the vanilla server replies even when empty. The vanilla
  server answers them on its main thread between ticks, so a probe arriving mid-tick
  waits for the tick to end.
  Against send phase that extra delay is a sawtooth, and fitting it gives the tick start
  and the work time W. The fit takes at least the last 20 s and follows the sawtooth's
  lower edge. why? on EMC about 30% of replies (pongs too) came 10–90 ms late, in clumps
  with other downstream packets (`set_health` spacing p10 9 ms, p90 78 ms in the
  2026-10-29 16:16 log, against ±4 ms on 10-28), and a symmetric fit failed on most 10 s
  windows of that log. The lower-edge fit found the edge in all 89 sliding 20 s windows
  (start 24–30 ms, W 17–26 ms). It needs W ≥ 8 ms; a proxy or plugin that answers
  completions off the main thread shows no jump at all.
- **Every reply leaves at the end of its tick.** 26.2's `MinecraftServer.tickChildren`
  suspends flushing for every player and flushes once after all levels and players have
  ticked. So `set_health` minus your move's send time minus the round trip is server wait
  + server work, not the wait alone; the readout falls back to that sum when the probes
  find no jump. In the 20:33 log of 2026-10-28, 7 of 8 kept bounces sat at a sum of
  15–21 ms and the 8th at 39 next to a stall: the perfect-bounce edge is the tick start,
  so that sum there is W.
- **Client wait and tick interval need a packet the server sends every tick.** They time
  `ClientboundSetHealthPacket`, which vanilla sends from `ServerPlayer.doTick` only when
  health or food changed. EMC sends it every tick (2026-10-28). Without it, client wait
  is derived from the loop (marked so) and the loop check is skipped.
- **The mod sends its own pings, 4 a second, while whitelisted.** why? vanilla pings only
  with the F3 network chart open, and the vanilla server answers
  `ServerboundPingRequestPacket` on the Netty thread, not at a tick, so it gives a clean
  round trip (63–71 ms on EMC, 2026-10-28 20:33).
- **The client tick grid is fixed for the whole process.** `DeltaTracker.Timer` keeps a
  fractional residual and drops extra catch-up ticks rather than resetting, so a hitch
  or a relog doesn't move it; a game restart does, and so do the buttons in Mod Menu →
  Tick phase. Those work only while not in a world (a server could see the phase move
  within a session) and only delay ticks (-1 ms is a 49 ms delay), so ticks never come
  closer than 50 ms. The tracker reads the grid from the
  timer. why? tick starts trail the grid by up to a frame (0–10 ms at ~100 fps), which
  would otherwise show up as phase noise.
- **Keep client wait and server wait away from 0/50.** A packet that arrives within a few
  ms of a tick edge is read on either side of it, so the glide-off reaches the client a
  tick early or late at random. In the 2026-10-29 17:05 log, at client wait ~0 the
  client first saw it 3 ticks after landing 112 times and 4 ticks 86 times, decided on
  the client (handled a median 0.2 ms before a tick, or 9 ms after one). A redeploy press
  came before the glide-off in 22% of the 3-tick bounces and 44% of the 4-tick ones.
  At client wait ~32 that split went away, but server wait ~47 then read 29 of 148
  landings a tick early. Server wait just under 50 is also where perfect bounces happen
  without lag (see the ebounce entry under "Things that will bite").
- **A packet-delay mod moves the waits, but only its hold mod 50 helps.** Ping Equalizer
  held moves ~68 ms and replies ~71 ms (2026-10-29 17:05 and 17:17 logs). The holds sit
  between the logger's send and wire records and between decode and handle, so net up +
  down leaves them out, while server wait and client wait include them: they dropped by
  18 and ~21 ms, as the loop says. The whole ticks of a hold are pure cost: the glide-off
  reached the client 6–7 ticks after landing instead of 3–4. The phase buttons cost
  nothing but move the two waits only against each other (client wait +d, server wait
  −d); their sum mod 50 is set by the round trip, the work on both ends and any hold.

## Playback (`/etas play`)

`/etas play PLAN [runs=N] [settle=T] [maxfail=K]` flies `<profile>/elytra-tas/plans/PLAN.tas`
(solver/PLAN-FORMAT.md version 2 or 1, or a plain `.tas` tape) as `tas_play` does: reset
with the slot-1 item, wait for the exact start pose (and the glide flag off), `settle`
still ticks (default 10), then line 1; again after "Course complete!" plus 20 ticks or an
abort (leaving the plan after "Course complete!" counts as done, not as an abort), until N runs are done (default: until stopped) or K attempts in a row failed
(default 3). `/etas stop` stops and hands the keyboard back; `/etas status` shows the
line, segment, divergences, reactions and the last attempts with the segments they flew.
While it plays, keys and mouse don't move you. The logger records it as `P` records
(netlog.py). Mod 0.5.0; 0.4.0 flies version 1 only. Tests: solver/oracle/README.md,
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
- **It refuses unless the account is `deftenvy`,** the plan's `account=` (if any) matches,
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

## Things that will bite

- **On EMC, vines end the glide.** Vanilla's `can_glide_through` block tag exempts vines
  (and cave, weeping and twisting vines) from the climbable check in `travelFallFlying`,
  but EMC sends no such tag (Burrowed Grove and canyon replays, checked 2026-11-02), and
  the client uses the server's tags. World files carry the server's tags for this
  reason; don't use vanilla's.
- **Flashback wraps yaw.** `Recorder.Position` stores `Mth.wrapDegrees(yaw)`; the
  client's yaw is unbounded, and elytra physics sees its float bits through a sine
  table, so 179.27 and -180.73 don't fly the same (~1e-5 blocks/tick). The extractor
  unwraps from the reset teleport's absolute yaw. why? without it the replay drifts
  off the recording within a few hundred ticks.
- **Don't use Minescript's `player_set_orientation` / `player_press_*` for playback.**
  The first applies `% 360` to yaw (same bit problem); the second defers through
  `set_timeout` to a render frame, so key changes can land a tick late. The player sets
  `Entity.setYRot/setXRot` and swaps `player.input` for a plain `ClientInput`. why?
  playback is open loop, so a one-tick slip or a changed float desyncs the run.
- **Flashback doesn't record keys or server teleports.** Inputs are fitted from the
  trajectory; teleports show up as pose jumps (`ClientboundPlayerPositionPacket` is in
  Flashback's ignored set).
- **A swapped-in `ClientInput` sees jump presses a tick early.** `LocalPlayer.aiStep`
  reads `wasJumping` from `input.keyPresses` before `input.tick()`, which in vanilla
  loads the new keys; a plain `ClientInput` has an empty `tick()`, so a jump set between
  ticks is never an edge and the glide never starts. The player schedules presses
  through `autoJumpTime` (vanilla then calls `makeJump()` after `tick()`). why? the
  elytra start is a jump edge in air.
- **Pyjinn can't assign fields of Java objects** (`player.input = x` fails at runtime,
  even for public fields); use `getField`/`getDeclaredField` + `Field.set`.
- **The local player's camera reads the live rotation, without interpolation**
  (`LocalPlayer.getViewYRot`), so setting rotation once per tick looks 20 Hz.
  `tas_play.pyj` sets the exact tape value in the tick handler and lerps only after
  physics (`END_CLIENT_TICK`, render events). why? physics and the server must only
  ever see tape values; a display value set in the tick handler gets flown (it did:
  0.5 blocks off by tick 56). All render events (Minescript's and Fabric's) fire after
  camera setup, so a value set in one shows up a frame later.
- **Pyjinn can implement Java interfaces** (`StartTick(fn)` makes a proxy), which
  reaches Fabric events Minescript doesn't wrap. Fabric listeners can't be removed;
  after exit Pyjinn no-ops them. An exception in one propagates into the game loop
  and crashes it, so catch everything and only set flags there.
- **Minescript's `chat` event misses server system messages** (Minescript 5.0b11, MC
  26.2): it hooks `ChatComponent.addClientSystemMessage`/`addPlayerMessage`, but server
  messages like EMC's "Course complete!" come through `addServerSystemMessage`.
  `tas_play.pyj` polls `ChatComponent.allMessages` instead. why? with the event, a
  completed lap was logged as "tape ended without completing".
- **Pyjinn's `range()` can't count down**: `range(n, -1, -1)` is empty (its iterator
  only tests `pos < stop`). Pyjinn 0.14.
- **Pyjinn floats are `Double`**, narrowed to `float` for method arguments but not
  constructor arguments: `Vec2(1.0, 0.0)` throws ClassCastException; use
  `Vec2(JavaFloat(x), JavaFloat(y))`.
- **Tick order (MC 26.2, Fabric API 0.160):** `START_CLIENT_TICK` → keybinds →
  Minescript's `tick` event (Fabric `START_LEVEL_TICK`, head of `tickEntities`) →
  player physics → `END_CLIENT_TICK`; then per frame: mouse turn → camera → render.
  Inputs set in the tick handler are used by the physics right after it, in the same
  tick. A pose Flashback records at tick N used the rotation recorded at N. why? the
  player is open loop, so anything set between the handler and physics is what flies.
- Gliding is client-side physics, keys are ignored, and only the server ends a glide
  (`LivingEntity.updateFallFlying` is server-only). An ebounce keeps the glide ("perfect")
  only if the server handles the landing move (onGround=1) and the jump move (onGround=0)
  with no player `doTick` between them; the server handles packets between ticks and checks
  onGround once per tick. But while it is behind schedule it runs a queued packet only once
  the packet is over 3 ticks old (26.2 `MinecraftServer.shouldRun`), so after a long tick
  the moves pile up and are handled together. Without lag both moves have to arrive in
  one tick's gap: the landing just after a tick start (server wait just under 50 ms) and
  the jump less than that later. Client ticks run at frame starts, so the two moves leave
  up to a frame less than 50 ms apart. In the 2026-10-29 17:05 log, at server wait
  ~47 ms (set by a 68 ms outbound hold, see Tick phase), 9 of 148 bounces kept the glide,
  8 of them with a flat round trip: 7 of the 53 whose moves left 45–48 ms apart, against
  2 of the 97 at 49 ms or more. At server wait ~29 (17:17 log) none of 246 did. Every other perfect bounce
  so far came with lag:
  on EMC all 5 perfect bounces in 312 landings (logs of 2026-10-20 01:41–02:06) fell in or
  beside server lag spikes (ticks of 91–725 ms), and the one singleplayer one in a client hitch that sent the two
  moves 27 ms apart. In the 2026-10-29 16:32 log, 6 of 9 kept glides in 132 bounces came
  during two freezes of the whole server (3.2 and 2.5 s: no set_health and no pongs), and
  the other 3 next to `set_health` gaps of 100–140 ms. Their landings' server waits
  (37–42 ms) were the same as the other bounces'. So most recorded runs lose the glide
  somewhere; the extractor writes the server's glide-offs into the tape as `@server`
  lines, and `etas verify` replays them.

  ![A normal bounce and the two ways a bounce comes out perfect](docs/ebounce.svg)
- Packet IDs in `flashback.py` are for protocol 776 (MC 26.2), read off `GameProtocols`
  in the client jar; 774 (MC 1.21.11) is mapped onto them, and any other protocol is
  rejected at load. `world.py` takes only 776, as its block table is 26.2's. A 774 replay's
  flight reproduces under port: all 5 finished academy runs of the 1.21.11 replay of
  2026-11-04.
- **A tape's pre-drop can be unflyable on our client.** Sprint from a double-tapped W is
  vanilla, and port does it; that replay's `W W - - - W` jumps didn't sprint. Its mod list
  has Tweakeroo, which can disable it (suspected, not tested). So `etas flight` starts seeds
  in the air.

## EMC course rules (measured)

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
  log reproduce exactly, 5 of them a tick or two late (lag); `python3 boosters.py scan`
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
- **Timer** is wall-clock time between the "Timer started!" and "Course complete!"
  server ticks, rounded to 0.02 s: five identical 647-tick runs gave 32.34 or 32.36
  depending on whether that interval was under or over 32.35 s.
- **The timer stops at the action bar's blank line, not at the chat.** While it runs, the
  bar shows `Time: X   Best Time: Y` every tick, and a blank line in the tick it stops; that
  tick matches the shown time. How late "Course complete!" comes depends on the account, in
  every replay up to 2026-11-04: `deftenvy`, 0 ticks on 676 of 677 finishes; `lqmbda`, 3-5 on
  most of 225; another player's 1.21.11 client, 3-5 on all 5. Perhaps a leaderboard write, as
  `deftenvy` is barred from the boards (a guess). `fit_course.py` fits to the bar. why? timing
  a non-blacklisted account's finish from its chat puts the crossing 3-5 ticks too late.
- **Start area** is box-like in bulk (canyon x < −266, 168 ≤ y < 177–178, z ≥ 164;
  super classic downtown −52 ≤ x < −27, 86 ≤ y < 99, z < −64) but not one box: near
  canyon's z ≈ 195–199 corner the −x face sits near −265.2, and landing at y = 167
  started the timer after 300 ms airborne below 168 did not. Leaving it without gliding
  starts the timer and kills you in the same tick.
- **Canyon finish** is no single box, sphere or cylinder under any delay; probably a
  union of boxes.
