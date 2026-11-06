# elytra-tas

Plan and fly elytra runs on EMC, from Flashback recordings to `/etas play`.

```
python3 tools/extract_run.py REPLAY.zip --list              # completed runs in a replay
python3 tools/extract_run.py REPLAY.zip -o NAME.tas         # last completed run -> tape
/etas play NAME runs=1                                      # in game, on deftenvy
```

Flashback replays live in `<profile>/flashback/replays/`, plans in
`<profile>/elytra-tas/plans/`.

`tools/` is flat, so its modules import each other; run them from anywhere.

- Readers:
  - `flashback.py`: Flashback replay reader (chunk/action format, local player poses,
    chat/title text, shared flags, held slot)
  - `players.py`: every player a replay saw, one `Life` per spawn; its docstring says how
    remote poses differ from local ones (`python3 tools/players.py REPLAY.zip [--dimension D]`)
  - `world.py`: the blocks the client had at a replay tick (block updates applied,
    unknown space explicit, `min_y` from registry data)
  - `netlog.py`: reader for the mod's logs; its docstring is the format spec
    (`python3 tools/netlog.py LOG.tsv.gz --timeline`)
  - `physics.py`: MC 26.2 elytra physics, float-faithful, for `extract_run.py`'s check (needs numpy)
- Extractors:
  - `extract_run.py`: finds runs, reconstructs per-tick inputs, writes a `.tas` tape and
    checks every gliding tick against elytra physics
  - `log_tape.py LOG --tick C`: a tape from a logger log's attempt, its real inputs (`extract_run.py` fits them from a replay)
  - `probe_timer.py QUEUE/done`: where the timer fired on each probe flown by `/etas queue`
  - `extract_world.py REPLAY.zip --tick T [--bounds X0 Y0 Z0 X1 Y1 Z1] -o OUT.world.gz`:
    writes a world file for `solver/` and prints a coverage report; its docstring is the
    format spec
  - `export_save.py COURSE_DIR/course.json -o SAVE_DIR`: converts the course's cached
    blocks to a Minecraft 26.2 singleplayer save, opening in spectator mode at the
    start. Unrecorded terrain is void; entities, sign text, and EMC course logic
    aren't included. Refuses to overwrite an existing save.
  - `segments.py COURSE_DIR REPLAY... [--tapes]`: cuts every player's attempts out of those
    replays and times them against the course's tape, into `runs/segments.jsonl.gz` and
    `runs/splits.md` (start-area exits, best stretch between gates, who covered what);
    `--tapes` writes local finished runs to `runs/seeds/`. Remote players' stretches are
    rough; see its docstring.
- Course fitting and checks:
  - `fit_course.py COURSE -o OUT.json`: fits a course file (EMC's rules for one map) from
    replays and logs; its docstring is the format spec
  - `check_runs.py`: checks that every completed run in the replays is in the model
  - `boosters.py scan`: measures boosters from logs and replays
  - `roughness.py`: how rough a trajectory's rotation is, to compare two builds of a plan
  - `route_hybrids.py`: seeds for trying a course's other routes (academy's stretches)
  - `block_dump.sh`: regenerates `data/blocks-26.2.tsv.gz` from the MC 26.2 jar

## Network/tick logger (`mod/`)

Logs what Flashback can't: exact client tick poses and keys, key presses and releases
between ticks, every outbound packet,
inbound packet decode and handle times, socket read/write times, and in singleplayer
the integrated server's view (move packets processed, player ticks, glide ends). One
gzipped TSV per connection in `<profile>/elytra-tas/logs/`. It starts on joining a
server or world (config via Mod Menu, YACL), or with an unbound toggle key. It only
observes, apart from the pings and probes of `/etas phase` and the playback of `/etas
play` below, so it may run on `lqmbda` too (playback allows `deftenvy`, plus `lqmbda` in singleplayer). Those go out only for whitelisted `user@server` pairs (Mod Menu →
Whitelist; `deftenvy@elytramc.net` by default, subdomains included, and singleplayer
always). why? they are packets a vanilla client wouldn't send. With the [undeploy](../undeploy) mod installed it
also records that mod's decisions as `U` records, since the packets it rewrites or makes
up don't show what the client saw.

```
cd mod && ./gradlew build
cp build/libs/elytra-tas-0.5.3.jar "$HOME/Library/Application Support/ModrinthApp/profiles/main/mods/"
```

Rename the previous version's jar in `mods/` to `.disabled`, as with emc-replay, and
restart Minecraft.

## Playback (`/etas play`)

`/etas play PLAN [runs=N]` flies `<profile>/elytra-tas/plans/PLAN.tas` in a loop on
`deftenvy` (plus `lqmbda` in singleplayer); `/etas stop` hands the keyboard back and
`/etas status` shows progress. `/etas queue on` flies plans a program drops in a folder. Options and behavior: [docs/playback.md](docs/playback.md).

## Docs

- [docs/emc-course-rules.md](docs/emc-course-rules.md): EMC course rules, as measured: boosters,
  wall-hit echoes, the timer, region faces, start areas
- [docs/ebounces.md](docs/ebounces.md): when a bounce keeps the glide, and how often
- [docs/tick-phase.md](docs/tick-phase.md): `/etas phase`, its intervals, and how client wait and
  server wait affect bounces
- [docs/playback.md](docs/playback.md): `/etas play`: its options, branching, refusals, when inputs land
- [docs/logger.md](docs/logger.md): what the logger's times mean, checking mixins without a client
- [docs/things-that-bite.md](docs/things-that-bite.md): vines on EMC, Flashback's yaw, tick order,
  packet IDs, sprint window
- [solver/README.md](solver/README.md): the solver toolkit
