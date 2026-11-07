# TODO

## uncategorized

- add instructions to use cif-cpu
- e2e tool to grab and optimize a run
- quantize rot to the sin table, so rather than checking approx equal, you check if they land in the same bucket
- consider wall touches
- partial ticks
- have agents read docs rather than the entire code for context management

## tasing strats / rules of thumb (move + collect the rest)

- have a notion of a binding block set
    - the blocks that relevantly constrain the tas
    - this is sparser than what floodfill would give, as we don't allow for non-myopic finding holes in the map to escape out of
    - so we can render just the binding blocks
    - and only consider binding blocks during optimization
- also the notion of a target ball we must pass though, for defining routes

## tools

- auto replay grab
    - i'll want the leaderboard replays for all the maps (and the map downloads themselves), which would be a lot of manual effort. and they'd get updated sometimes, as new runs get set (and the maps sometimes get updates). so we only watch a replay if it's not already in our library. also we can check if the replay/server lagged and rewatch that part (if it lagged near the end, we may have gotten kicked out of the replay, so we'd need to rejoin and skip forward). if you punch the player, you enter the player and get their pos/rot, but this doesn't load chunks as though you were there. so i think we shouldn't use that, and instead just use the teleport to player item repeatedly so we'll be loading chunks. but maybe the player will get out of render distance too often, which we'd need to patch by rewinding the replay, which can only be done in 10s increments, or play it back at .5 or .25 speed, which is slow for realtime. so ig we'd need to do both: be inside the target player, and then once they get close to exiting loaded chunks, we exit, pause, tp, maybe wait for chunks to load, and renter and play.
    - i think this should go in the etas mod. do you agree with this design?
    - i think for debugging you should be able to directly take actions with deftenvy on the server, are you able to do that?
    - i also want a way to automate finding the start and end regions, maybe by claudes submitting tases to probe. basically just claudes playing on the server with deftenvy.
- generally should be looking for rules of thumb / generalizable knowledge. maybe write a reflection on each tas. we don't yet know what optimal ebounces look like. or much about turning.
    - maybe also more tas-specific recipes. like how chatter converges slowly, so even if you're fine with chatter, you should first get a run without it.
    - what if instead of ablating the physics, we ablate the gradient? so we fly the unabated physics, but take gradients with the ablated physics. how different are the ablated physics anyway? it probably matters for the L corner step, but maybe not for other things.
- making critiques of runs? like which segments were bad? or take a bunch of runs and see where you're biased away from the optimum.

## solver

- plans that take damage on purpose. recordings already can (the server's velocity overwrite is an out-of-model event); plans can't, since the server computes damage on its own copy of the physics, which disagrees slightly, and it arrives a few ticks late at a time that varies
- fireworks, in the single-rocket mode
- route search: what proposes routes when no recording exists
- run `check_runs.py` to completion (it hadn't been as of 2026-11-04 19:40 EST)
- the second curvature ablation's hook
- designed but never needed yet: `Secant` collision (the segment between tick positions, for when the L's sub-tick detail costs more than it's worth), specialized kernels, direct shooting, the tunnel DP, multistart wrappers, a polyline sketch seeder

## experiments

- experiment with wall hits (to find a rule of thumb): at what point in a turn would it be nice for there to be a wall (ie to cancel our remaining vel on that axis)
    - somehow have a tighter turn, which should want the wall hit more
- dropdowns for academy?
- check: does higher ping mean you can glide along the ground (which should kill you) at the end? does the server stop the timer based on your position without checking if you should be dead. like does it send the packet to kill you, but still allow you to complete after this has been sent, st if you complete it before you die, it counts as not dying? (i ping spoofed on canyon for data) (also i got a flashback from luna in data). maybe make a TAS to sweep this automatically if there isn't enough data.

## tases

- natural tases
    - [Fastest Possible Elytra Horizontal Speed (75.1886m/s)](https://www.youtube.com/watch?v=hwMrtobXU-c)
        - we should replicate
        - and then optimize
            - max y vel (maybe this is uninteresting)
            - max xz vel (horizontal speed)
            - max xyz vel (speed)
            - maybe the thing that gave -40deg, it was something like TE/vy
        - and see if we can do better
        - certainly we can get there faster
        - "min time from v0=0 to max vel" is a natural thing to optimize
        - also compare against optimal steady state for reference
    - [Infinite Elytra Flight Without Rockets From 0 Initial Height With 1 Block Momentum (TAS)](https://www.youtube.com/watch?v=OjkYASNKCdE)
    - floor to build height on a superflat
    - floor to build height with 1bmm
    - min number of jumps until infinite flight
    - one jump oj
        - max height with a single jump
        - max height with a single sprint jump
        - max distance with a single sprint jump
        - max distance with a two jumps with 1bmm (like force momentum but elytra)
        - max distance with a two jumps with nbmm (like force momentum but elytra)
        - max distance with a n jumps with nbmm (like force momentum but elytra)
        - optimal nbmm setups with a constant pitch for the first jump (or first n jumps)
            - for human use
            - ref the setup on super classic downtown
        - facing a wall, jumping and pitching, max height
- something for hits?
- emc maps
    - academy
        - consider the three wall hits
    - canyon
    - monster
    - classic evergreen
    - classic ravine
    - speedchecker
        - is luna's 6.28 wr real or lagged?
    - super classic downtown
    - swashbuckle
    - firewatch
    - mexico
