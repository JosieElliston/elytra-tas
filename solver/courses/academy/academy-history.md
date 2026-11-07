# How the academy 527 happened

This is a history, not a recipe. It says what happened on one map, in the order it happened, including detours
and choices made under time pressure. Nothing here shows that this order is good or that these steps are needed.
The tools were changing throughout, and later tools may make most of it unnecessary.

The plan is `academy_527.tas`: 527 ticks, predicted 26.35 s. Its artifacts are under
`solver/courses/academy/runs/`, which is gitignored and local to one machine. Names below are those artifacts, and
it won't replay byte for byte: several steps ran on uncommitted code. All dates are 2026 and all times EST.

## The line of descent

Captured run to plan, with what the timed flight read at each stage. Each stage has other parents too.

| When | Ticks | What happened |
|---|---|---|
| 11-03, 11-04 | 633 | Replays captured. Luna's 31.58 is 633 ticks as recorded. |
| 11-04 23:45 | 540 | Solved each captured run with a sliding-window optimizer. Luna's 31.58 came out best. |
| 11-05 01:43 | 537 | Added a price on rotation, spliced in stretches of other attempts, and repolished `candles_68_117_rambells`. |
| 11-05 07:22 | 535 | Built routes between gates, then crossed the polished flights at those gates: `gen1_34`. |
| 11-06 00:30 | 530 | Replaced the way out of the start room with a takeoff off the slab: `strafe_32`. |
| 11-06 03:49 | 528 | Crossed flights where their states matched, at margin 0.012: `margin_cross2_0`. |
| 11-06 07:10 | 527 | Crossed and polished again, then finished the whole run: `f527_0.3`. |

## Stage by stage

### Captured runs

The seeds are 17 finished runs in `runs/seeds/`. Sixteen came from two replays:

- `2026-11-03T22_40_44_academy.zip`: 34.02 to 42.54, including lqmbda's 34.02.
- Luna's `data/foreign/luna/2026-11-04_academy.zip`: all 5 finished runs, 30.98 to 31.64.

Each was cut to its flight and optimized alone.

| Seed | Best result |
|---|---|
| Luna's 31.58 | 540 |
| Luna's 31.64 | 544 |
| Luna's 31.18 | 547 |
| Luna's 31.22 | 548 |
| 37.26 | 549 |
| lqmbda's 34.02 | 555 |

Luna's 30.98 was the fastest recorded, but its polish never reached the finish box and it dropped out here.

### First plans, 540 to 537

Polishing with the rotation price took 31.58 to 539. `tools/route_hybrids.py` then spliced stretches of other players'
attempts into flights. The stretch that stuck was rambells', at the candles, in a flight that keeps 31.58's first 19
rows. Repeated polishing took it through `again1`..`again3` to `pass4`, the 537. Two more polishes after the solver's
pitch range was widened gave `fullrange2`, still 537.

These plans reached the flight from the reset by replaying lqmbda's 34.02 up to its deploy and joining from there.
That prefix stayed in use through the 535.

### Routes between gates, 537 to 535

`routes` built value tables between gates across the course, seeded from recorded flights, recorded attempts and voxel
searches. Its stitched flights, polished, came back around 540 to 552 (`tight_*`, `first546`, `first552`).

`tools/route_splice.py` then took each stretch from whichever flight did it best, across `fullrange2` and those.
After another polish with `--server`, `cross_34` read 535 and was copied to `gen1_34`. That flight became the reference
for everything after it.

### The slab, 535 to 530

`dome` searched for flights that ebounced to start high up, but it turned out that the human strat of starting on the slab and jumping down without deploying was faster, as you could sprint forwards to gain forward speed, which didn't trade off downwards speed the way flying forwards does. `slabtime` searched the keys and timing off it, including sprints south and 45 degree strafes. `strafe_32` is
one of those takeoffs joined to `gen1_34`'s flight.

### Crossing on state, 530 to 528

`tools/route_cross.py` changed flights where position and velocity agreed, so gains on separate stretches could add up.
That gave `cross2_*` and `cross3_*`. Polishing at margin 0.012 instead of the usual gave the `margin_*` flights, and
`margin_cross2_0` read 528. Fractions of a tick were carried across windows too.

`slabland` then found an ebounce from the reset onto the slab, with a walk back to the takeoff spot for each arrival of
the glide-off. No recorded prefix is left in the plan from this point on, though its start state is still read from
lqmbda's 34.02.

### The 527

1. `ideas setup margin_cross2_0` and `ideas door margin_cross2_0` tried other ways off the slab and through the door.
   Polished under `--server`, they gave `idea_setup_north_d` and `idea_door_5`.
2. `route_cross.py` crossed 14 of those and the `margin_*` flights, giving `r2x_0`: `idea_door_5`, then
   `margin_cross2_10` from `@106`. Its first polish came back 529.
3. After a change to `optimize`'s first plan and a new `--plain`, 136 route seeds were polished again with
   `--hybrid --plain --server --margin 0.012`. `v_r2x_0` read 526.94 and timed 527 with no server hits. Next best was
   527.09, which times 528.
4. `ideas graft idea_setup_north_d v_r2x_0 g527` put the north setup on the new flight.
5. `ideas finish g527 60 f527_0.3 0.3` re-solved from tick 60 to the finish with rotation priced, and no windows.
   Weights 0.03 and 0.1 also timed 527.
6. `slabland join f527_0.3` joined the ebounce, and `etas verify` passed all 7 paths at 527.

## What isn't settled

- **Why step 3 found a 527.** Commit `7cc15eb` credits the first-plan fix. But `r2x_0` reads `first 528.95` in both the
  old and new logs. The old run then smoothed and stuck at 528.94, and the new one skipped smoothing and dropped to
  526.94. So `--plain` may have mattered instead. The two changed together and were never tested apart.
- **What came after.** Later work on partial ticks and collision models (`anneal`, chords, `--triple`, `--chain`) isn't
  in this plan, which dates from 07:10 on 11-06.
- **Stages with gaps.** Upstream of `strafe_32` and `cross2_*` this history rests on file headers and commit messages,
  and it doesn't trace every parent.
