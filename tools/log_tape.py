"""Cut a flyable .tas tape out of a logger log, for one attempt from the course's start pose.

usage: python3 tools/log_tape.py LOG.tsv.gz --tick C [--after N] [--course DIR] -o OUT.tas

C is any client tick of the attempt, usually a "Timer started!" tick. The tape runs from
the last tick the player sat at rest before C through C + N (default 12). Unlike
extract_run.py this needs no input fitting: T records hold each tick's real rotation and
keys, and their poses are bit exact.

A hand reset from mid-glide drifts a few mm, and playback starts only from the exact reset
pose, so a tape can't be flown as recorded. `--course DIR` writes the course's reset pose
in `start=`; then `etas tas2traj` and `etas traj2tas` refly the inputs from there under
port and give the poses `/etas play` will check.
"""
from __future__ import annotations

import argparse
import json
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import netlog


def fbits(v: float) -> str:
    return struct.pack(">f", v).hex()


def dbits(v: float) -> str:
    return struct.pack(">d", v).hex()


KEYS = ((1, "W"), (2, "S"), (4, "A"), (8, "D"), (16, "J"), (64, "R"))


def keys_of(bits: int) -> str:
    return "".join(c for b, c in KEYS if bits & b) or "-"


def cut(ticks: list[netlog.Tick], c: int, after: int, source: str, start: str | None = None) -> list[str]:
    """`ticks` holds the log's end-of-tick records in order. A tape from the rest before tick c.
    `start` is a `start=` value in place of the rest pose."""
    by = {t.tick: t for t in ticks}
    if c not in by:
        raise SystemExit(f"no T record for tick {c}")
    # the last tick still at rest, walking back from c
    t0 = c
    while t0 - 1 in by and by[t0].pos != by[t0 - 1].pos:
        t0 -= 1
    t0 -= 1
    rest = by.get(t0)
    if rest is None or rest.vel[0] != 0.0 or rest.vel[2] != 0.0 or not rest.on_ground or rest.fall_flying:
        raise SystemExit(f"tick {c}: no rest before it (reached tick {t0})")
    # any tick with a sneak can't be flown: plans have no sneak key
    rows = [by[t] for t in range(t0 + 1, c + after + 1) if t in by]
    if len(rows) != c + after - t0:
        raise SystemExit(f"tick {c}: T records have a gap")
    if any(r.input & 32 for r in rows):
        raise SystemExit(f"tick {c}: sneaks, which plans can't")
    x, y, z = rest.pos
    out = [f"source={source}", "start=" + (start or f"{dbits(x)} {dbits(y)} {dbits(z)} {fbits(0.0)} {fbits(0.0)}"), f"ticks={len(rows)}",
           f"source_timer_tick={c - t0}"]
    for i, r in enumerate(rows, 1):
        out.append(f"{i} {fbits(r.yrot)} {fbits(r.xrot)} {keys_of(r.input)} {dbits(r.pos[0])} {dbits(r.pos[1])} {dbits(r.pos[2])}"
                   f"    # {r.pos[0]:.4f} {r.pos[1]:.4f} {r.pos[2]:.4f}")
    return out


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("log")
    ap.add_argument("--tick", type=int, required=True)
    ap.add_argument("--after", type=int, default=12)
    ap.add_argument("--course", help="course dir: start the tape at its reset pose")
    ap.add_argument("-o", "--out", required=True)
    a = ap.parse_args(argv)
    start = " ".join(json.loads((Path(a.course) / "course.json").read_text())["start"]["pose"]) if a.course else None
    log = netlog.load(a.log)
    ticks = [t for t in log.ticks if t.phase == "e"]
    Path(a.out).write_text("\n".join(cut(ticks, a.tick, a.after, Path(a.log).name, start)) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
