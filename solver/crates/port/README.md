# port

The mechanical port of MC 26.2 client movement for the local player (ARCHITECTURE.md,
"Physics" item 1): `LocalPlayer`, `Player`, `LivingEntity` and `Entity`, plus the
`VoxelShape`/`Shapes` collision code, transcribed with the Java's casts, `Mth`'s sine
table and its call order. It answers one question: is this the Java. Don't optimize
or reorganize it; that is the faithful kernel's job.

`port::tick(&mut LocalPlayer, &TickInput, &level)` runs one client tick. `LocalPlayer`
holds the Java fields the ported methods use; each source file is one Java class, and
each function cites the method it transcribes. A level is anything implementing
`port::Level` (`world::World`, or `BlockMap` for hand-built tests).

## Testing

```
cd solver && CARGO_TARGET_DIR=target/agent-port cargo test -p port --release
```

- `tests/tape.rs`: the Burrowed Grove tape (`tas/burrowed_grove.tas`). The continuous
  replay from the start pose, with a flat floor under the start standing in for the
  course, is bit-exact for all 711 ticks before the finish landing (ground walk, jump,
  glide start, the `|v_y| < 0.003` zeroing at 555–559). Per-tick replay of the 695
  airborne ticks from the tape's poses is within 2.9e-14: the tape has no velocity,
  so it is derived from the poses as the extractor does. `tape_full_replay_with_world`
  replays all 715 ticks, the finish landing included, against the world crate's
  Burrowed Grove fixture: bit-exact, onGround included (2026-11-02).
- `tests/logs.rs`: four logger logs, read from the Modrinth profile
  (`ETAS_LOG_DIR` overrides; a missing log is skipped). Each airborne tick without
  collision flags is ticked from its START record and compared with its END record
  in an empty world. The test's `LOGS` comment says which logs must match fully and
  what the others' mismatches are (creative flight, a roof, a firework).
- `tests/collision.rs`: box worlds for axis order, corner clipping, step-up onto a
  slab, landing, and the fluid and unknown-space errors.

`BlockMap` blocks get the table row `level::block_detail` implies (class `Block`,
vanilla's defaults); use `set_with_detail` for anything else.

## What it does not do

Each of these is an `Unsupported` error rather than a silent wrong answer, unless
marked fixed:

- Fluids, climbables, stuck blocks (cobweb, powder snow), bouncing blocks (slime,
  beds) and unknown space are errors, read from `world::BlockDetail`. Climbing uses
  the world file's tags: a `can_glide_through` block is flown through while gliding,
  as in the game, and EMC sends no such tag, so there vines end the glide.
- Fixed: survival or adventure, no effects, no item in use, food above 6, no
  vehicle, auto-jump off, default attributes, sprint window 7. Creative flight,
  swimming, riding are not ported.
- No entity collisions (boats, shulkers) and no world border.
- The glide ends only when the server says so, and boosters and the wall-hit echo
  are the server's velocity: the caller applies those packets before a tick, with
  `LocalPlayer::handle_glide_off(pose)` and `handle_set_entity_motion(v)`
  (`client_packet_listener.rs`; `etas::server::apply` picks the glide-off's pose).
  Wall damage, fireworks and knockback are not here.
- `isSuffocating` (for `moveTowardsClosestSpace`) is the table's value at an empty
  origin; world-dependent cases (shulker boxes) aren't covered.
- Block shapes are rebuilt from the block table's boxes as `VoxelShape.optimize()`
  would. The coordinate grid of a shape matters (collision scans from the cell
  holding the box face; step-up tries every grid height), so a block whose game
  shape isn't the optimized union of its boxes could differ.
- The fall-distance reset from `level.clip` on moves of a block or more is left out;
  fall distance only matters here for sneaking at ledges.

## Things that will bite

- **Input is `KeyboardInput`'s:** keys are read inside `aiStep` (`input.tick()`), so
  `wasJumping` and the double-tap-sprint check see the previous tick's keys, held in
  `LocalPlayer.input`. `tas_play` swaps in a plain `ClientInput` and makes jump edges
  through `autoJumpTime`; set `auto_jump_time` to model that.
- **The logger's START record carries the previous tick's keys**, its END record this
  tick's; the rotation physics used is the END record's. The pose isn't logged.
- **The lift's `Math.cos` is fdlibm (`mth::java_cos`), not `f64::cos`.** why? macOS
  libm differs from Java in the last place at about 4% of pitches, and that moved the
  Burrowed Grove replay from tick 493. fdlibm isn't the game either: the game's JVM
  (HotSpot on arm64) differs from it at 0.55% of float pitches, which the oracle sees as
  1-ulp velocity mismatches on 3 of 9455 ticks (2026-11-03).

## Java sources

Decompiled with `./gradlew genSources` in `mod/` (Fabric Loom, Mojang mappings,
Vineflower); the sources jar it prints is under `mod/.gradle/loom-cache/`. The
transcribed classes are in `net/minecraft/`: `client/player/{LocalPlayer,
KeyboardInput,ClientInput}`, `world/entity/{Entity,LivingEntity,Avatar,
EntityDimensions}`, `world/entity/player/Player`, `world/phys/{Vec3,Vec2,AABB}`,
`world/phys/shapes/*`, `world/level/{BlockCollisions,CollisionGetter}`, `util/Mth`.
