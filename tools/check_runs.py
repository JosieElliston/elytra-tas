"""Check that every completed run in the replays is in the model.

usage: python3 tools/check_runs.py [--replays GLOB] [--out DIR] [-j N] [--etas BIN] [--list]

Each run `extract_run.py` finds becomes a tape. On a map with a course (a course's
`dimension`, `solver/courses/*/course.json`) `etas replay --course` replays it under port
and the faithful kernel and runs `verify`. On any other map it replays in a world taken
from the same replay, unloaded columns kept. A run fails if port or the faithful kernel
don't reproduce it or a course rule says it couldn't have finished: the server let it
finish, so a failure is a gap in the model, not a verdict on the run.

Writes OUT/runs.tsv (replay, run, map, ticks, reset, finish, glide-offs, velocities,
teleports, damage events, extraction problems, result, first failure), OUT/tapes/ and
OUT/worlds/, and prints the totals and every failure. OUT defaults to
~/.cache/elytra-tas/check_runs (generated, large, and not for the repo). Exit 1 if a run
fails. `--list` stops after the extraction.
"""

from __future__ import annotations

import argparse
import glob
import json
import subprocess
import sys
from collections import Counter, defaultdict
from multiprocessing import Pool
from pathlib import Path

import extract_run as er
import extract_world as ew
import flashback as fb
import world as wd

ROOT = Path(__file__).resolve().parents[1]
PROFILE = Path.home() / "Library/Application Support/ModrinthApp/profiles/main"
REPLAYS = PROFILE / "flashback/replays"
COURSES = ROOT / "solver/courses"
MARGIN = 16  # blocks of world around a map's poses
BATCH = 64  # tapes per `etas replay`


def courses() -> dict[str, Path]:
    """dimension -> course directory."""
    out = {}
    for p in COURSES.glob("*/course.json"):
        out[json.loads(p.read_text())["dimension"]] = p.parent
    return out


def dimension_at(r: fb.Replay, tick: int) -> str | None:
    cur = None
    for e in r.events:
        if e.kind == "dimension" and e.tick <= tick:
            cur = e.data
    return cur


def scan(job) -> list[dict]:
    """One replay: its tapes, and a world for each map no course covers."""
    path, out, course_dims = job
    r = fb.Replay.load(path)
    rows, groups = [], defaultdict(list)
    damage = sorted(e.tick for e in r.events if e.kind == "damage")
    for i, run in enumerate(er.find_runs(r)):
        dim = dimension_at(r, run.finish)
        tape = out / "tapes" / Path(path).stem / f"run{i}.tas"
        tape.parent.mkdir(parents=True, exist_ok=True)
        row = dict(replay=Path(path).name, run=i, map=(dim or "?").removeprefix("minecraft:"), dim=dim,
                   reset=run.reset, finish=run.finish, tape=str(tape), text=run.finish_text, problems=0,
                   damage=sum(run.reset < t <= run.finish for t in damage), result="", why="")
        try:
            header, lines, problems, _ = er.extract(r, run)
            tape.write_text("\n".join(header + lines) + "\n")
            row["problems"] = len(problems)
            row["first_problem"] = problems[0] if problems else ""
        except Exception as e:  # noqa: BLE001 - a run that won't extract is a result
            row["result"], row["why"] = "extract", f"{type(e).__name__}: {e}"
        rows.append(row)
        if dim not in course_dims:
            groups[dim].append((run, row))
    for dim, members in groups.items():
        last = max(run.finish for run, _ in members)
        pts = [p for run, _ in members for t, p in r.poses.items() if run.reset <= t <= run.finish + er.TAIL_TICKS]
        lo = [int(min(getattr(p, a) for p in pts)) - MARGIN for a in "xyz"]
        hi = [int(max(getattr(p, a) for p in pts)) + MARGIN for a in "xyz"]
        file = out / "worlds" / f"{Path(path).stem}.{(dim or 'none').replace(':', '_')}.world.gz"
        file.parent.mkdir(parents=True, exist_ok=True)
        if not file.exists():  # a replay's blocks don't change; delete OUT/worlds to rebuild
            w = wd.World.from_replay(r, last, keep_forgotten=True)
            ew.write_world(file, w, r.path.name, last, [lo, hi])
        for _, row in members:
            row["world"] = str(file)
    wd.scan_replay.cache_clear()
    return rows


def replay_batch(job) -> list[dict]:
    etas, key, tapes = job
    kind, where = key
    cmd = [etas, "replay", f"--{kind}", where, *tapes]
    p = subprocess.run(cmd, capture_output=True, text=True)
    if p.returncode != 0:
        return [dict(tape=t, error=f"etas replay: {p.stderr.strip()[:300]}") for t in tapes]
    return [json.loads(line) for line in p.stdout.splitlines()]


def judge(row: dict, c: dict) -> None:
    row.update(ticks=c.get("ticks", 0), glide_offs=c.get("glide_offs", 0), velocities=c.get("velocities", 0),
               teleports=c.get("teleports", 0), hits=len(c.get("hits") or []),
               max_dev=c.get("port_max_dev", 0.0))
    why = []
    if c.get("error"):
        why.append(f"error: {c['error']}")
    if c.get("port"):
        why.append(f"port tick {c['port'][0]}: {c['port'][1]}")
    if c.get("faithful"):
        why.append(f"faithful tick {c['faithful'][0]}: {c['faithful'][1]}")
    why += [f"course: {f}" for f in c.get("course") or []]
    row["result"] = "FAIL" if why else "pass"
    row["why"] = " | ".join(why)


COLUMNS = ("replay run map ticks reset finish glide_offs velocities teleports damage hits problems result why").split()


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--replays", default="*.zip", help=f"globs under {REPLAYS}, comma-separated")
    ap.add_argument("--out", type=Path, default=Path.home() / ".cache/elytra-tas/check_runs")
    ap.add_argument("-j", type=int, default=8)
    ap.add_argument("--etas", default=str(ROOT / "solver/target/release/etas"))
    ap.add_argument("--list", action="store_true", help="only extract and list")
    args = ap.parse_args(argv)

    args.out.mkdir(parents=True, exist_ok=True)
    dims = courses()
    paths = sorted(p for g in args.replays.split(",") for p in glob.glob(str(REPLAYS / g)))
    with Pool(args.j) as pool:
        rows = [row for rs in pool.imap(scan, [(p, args.out, set(dims)) for p in paths]) for row in rs]
        if not args.list:
            groups = defaultdict(list)
            for row in rows:
                if row["result"] == "extract":
                    continue
                key = ("course", str(dims[row["dim"]])) if row["dim"] in dims else ("world", row["world"])
                groups[key].append(row["tape"])
            jobs = [(args.etas, k, ts[i:i + BATCH]) for k, ts in groups.items() for i in range(0, len(ts), BATCH)]
            by_tape = {c["tape"]: c for cs in pool.imap_unordered(replay_batch, jobs) for c in cs}
            for row in rows:
                if row["result"] != "extract":
                    judge(row, by_tape.get(row["tape"], dict(error="no result")))
    with open(args.out / "runs.tsv", "w") as f:
        print("\t".join(COLUMNS), file=f)
        for row in rows:
            print("\t".join(str(row.get(c, "")) for c in COLUMNS), file=f)
    if args.list:
        for row in rows:
            print(row["replay"], row["run"], row["map"], row["reset"], row["finish"], row["problems"], row["text"])
        print(len(rows), "runs")
        return 0
    per = defaultdict(Counter)
    for row in rows:
        per[row["map"]][row["result"]] += 1
    for m, c in sorted(per.items()):
        print(f"{m}: {dict(c)}")
    fails = [row for row in rows if row["result"] != "pass"]
    for row in fails:
        print(f"{row['replay']} [{row['run']}] {row['map']} {row.get('ticks', '?')} ticks: {row['why'][:300]}")
    print(f"{len(rows)} runs, {len(rows) - len(fails)} pass, {len(fails)} fail; table in {args.out / 'runs.tsv'}")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
