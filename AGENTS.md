# AGENTS

this folder is for the elytra tas project.
we'll be tasing on the elytra mc / emc server.
we'll be tasing on my alt `deftenvy`, *not* my main `lqmbda`.
i have admin approval for doing this; my alt was given a competitive ban so that i can tas with it.
the initial work will likely be done with [minescript](/Users/josie/Library/Application Support/ModrinthApp/profiles/main/minescript).
any script that modifies player state should assert that the active account is my alt.
scripts that don't take player actions, like the replay grabber, are fine to run on my main.

[README.md](README.md) has the tools and the constraints that bite (Flashback yaw wrapping, Minescript input APIs to avoid, tick order).

see also

- [elytrasim](/Users/josie/programming_local/elytrasim): where the theory work is
- [elytra-vario](/Users/josie/programming_local/elytra-vario): a mod that renders a hud with various results from the theory work.
- [emc-replay](/Users/josie/programming_local/emc-replay): a mod for rendering replays we grab with minescript from emc.
- [undeploy](/Users/josie/programming_local/undeploy): a public client mod that ends the elytra glide a fixed N ticks after landing (`/undeploy`); the logger records its decisions as `U` records.
