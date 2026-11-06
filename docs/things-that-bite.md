# Things that will bite

Game and tooling facts that aren't specific to one part of the repo.

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
- **Pictures leave out most barriers, and barriers still collide.** `etas view` draws only
  those within 10 blocks of a run, solid, and `WorldStyle` none, since a hull of them around
  a map would tint every view (academy has 2.8M cells of one, as of 2026-11-05). Some
  farther ones matter: start gates, walls that block a skip. Before concluding a new route
  is open, check with `etas view --barriers` or `verify`, not by eye.
- **Pictures cut blocks away, and cut blocks still collide.** `etas view` hides what stands
  between each camera and the path, so each panel is missing different blocks, and a gap
  in one isn't a gap in the world. Check a route's clearance with `verify`.
- **Tick order (MC 26.2, Fabric API 0.160):** `START_CLIENT_TICK` → keybinds →
  Minescript's `tick` event (Fabric `START_LEVEL_TICK`, head of `tickEntities`) →
  player physics → `END_CLIENT_TICK`; then per frame: mouse turn → camera → render.
  Inputs set in the tick handler are used by the physics right after it, in the same
  tick. A pose Flashback records at tick N used the rotation recorded at N. why? the
  player is open loop, so anything set between the handler and physics is what flies.
- Packet IDs in `flashback.py` are for protocol 776 (MC 26.2), read off `GameProtocols`
  in the client jar; 774 (MC 1.21.11) is mapped onto them, and any other protocol is
  rejected at load. `world.py` takes only 776, as its block table is 26.2's. A 774 replay's
  flight reproduces under port: all 5 finished academy runs of the 1.21.11 replay of
  2026-11-04.
- **Sprint window is 0 everywhere; `/etas play` and `tas_play` refuse any other.** It's a
  client option (Controls, 0-10, vanilla default 7), and at 0 a double-tapped W never
  sprints. why? a recorded tape's sprints come from its sprint flag as `R`, whatever window
  that player used, so port needs no window of theirs: at 7, `refit` had to change keys in
  2 of Luna's 5 academy runs (W presses it took for a double tap) and at 0 in none, and
  901 runs of our own replays gave the same results at both (2026-11-04 21:50 EST).
- **A tape's pre-drop can be unflyable on our client,** so `etas flight` starts seeds in the
  air.
