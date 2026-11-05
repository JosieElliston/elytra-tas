# Minescript scripts

As of 2026-11-04 22:05 EST, Minescript/Pyjinn is mostly superseded by the mod (`mod/`).
They need the symlinks in `<profile>/minescript/` to run; logs go to `minescript/tas_logs/`
there. The leaderboard replay grabber (`emc_replay_grab.pyj`) lives in that folder, not here,
and is still the way to grab replays.

- `tas_play.pyj`: superseded by `/etas play`. Last used 2026-10-20. Its docstring has the
  tick-level reasoning `Playback.java` inherited.
- `undeploy_test.pyj`: tests the [undeploy](../../undeploy) mod on EMC: `/hub`, then
  ebounces in place under undeploy off and several N, and logs each landing's glide-off ticks
  and any glide lost in the air (`\undeploy_test`, on deftenvy). Closed loop, so `/etas play`
  doesn't replace it.
- `pyjinn_check.sh FILE.pyj...`: compiles `.pyj` files with the Pyjinn inside the installed
  Minescript mod, as `\script` does. Run it after every edit. why? Pyjinn accepts less than
  Python (e.g. `S["a"], S["b"] = 1, 2` is rejected: only plain names in tuple targets),
  CPython's `ast` passes those, and in game you see only the first error per launch. It
  catches syntax only, not runtime errors.

## Things that will bite

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
