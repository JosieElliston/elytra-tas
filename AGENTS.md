# AGENTS

this folder is for the elytra tas project.
we'll be tasing on the elytramc.net / emc server.
on emc, we'll be tasing on my alt `deftenvy`, *not* my main `lqmbda`.
i have admin approval for doing this; my alt was given a competitive ban so that i can tas with it.

the initial work was done with [minescript](/Users/josie/Library/Application Support/ModrinthApp/profiles/main/minescript), now mostly superseded by the mod (`mod/`, `/etas play`).
any script that modifies player state should assert that the active account is my alt, except in singleplayer.
scripts that don't take player actions, like the replay grabber, are fine to run on my main.

you can use `ssh cif-cluster` for compute.

to look at a run against the world's blocks, use the `view` crate (`solver/crates/view`)

## emc timer

the emc timer is (server-side) realtime, rounded to 0.02s.
so, with negligible lag, a 20 tick run is 1.00, a 22 tick run is 1.10,
but a 21 tick gives 1.04 or 1.06 at random.
we also see times like 1.02 or 1.08,
which means there was >10ms of error.
these should generally be considered as actually being 20 / 22 ticks,
but it's also possible that there was >20ms of error and it was actually 21 ticks.
the timer begins on the first move that changes your block while your previous position was already outside the start region (the end is symmetric).

## see also

- [elytrasim](/Users/josie/programming_local/elytrasim): where the theory work is
- [elytra-vario](/Users/josie/programming_local/elytra-vario): a mod that renders a hud with various results from the theory work.
- [emc-replay](/Users/josie/programming_local/emc-replay): a mod for rendering replays we grab with minescript from emc.
- [undeploy](/Users/josie/programming_local/undeploy): a public client mod that ends the elytra glide a fixed N ticks after landing (`/undeploy`); the logger records its decisions as `U` records.
