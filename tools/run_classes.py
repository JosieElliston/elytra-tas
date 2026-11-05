#!/usr/bin/env python3
"""Attempts of a course sorted into classes by where they touched the ground before the timer started.

usage: python3 tools/run_classes.py FLOORS.jsonl SEGMENTS.jsonl.gz... [--center X Z] [--gate N] [--json OUT]

FLOORS.jsonl is the course's `floors` bin's output, SEGMENTS `segments.py`'s. A touch is a tick that
starts on the ground, and its label a floor component and, for a component wider than one sector,
which side of `--center` it's on, so a ring around a hole isn't one place.
A class is the sequence of labels with repeats merged, and the touch count is kept beside it: walking
is one touch, and each jump's landing another.

Per class: attempts, how many got out, the highest point before the exit, exit speed, and the ticks
from the exit to gate N (remote players' are good to a tick or so, see segments.py).
"""

from __future__ import annotations

import argparse
import collections
import gzip
import json
import math
import statistics

# components at most this wide in blocks get no sector
ONE_PLACE = 8


def load_floors(path):
    cells = {}
    extent = collections.defaultdict(lambda: [1e9, -1e9, 1e9, -1e9])
    for line in open(path):
        c = json.loads(line)
        cells.setdefault((c["x"], c["z"]), []).append(c)
        e = extent[c["c"]]
        e[0], e[1] = min(e[0], c["x"]), max(e[1], c["x"])
        e[2], e[3] = min(e[2], c["z"]), max(e[3], c["z"])
    return cells, extent


def sector(x, z, center):
    """Which side of the center: n (-z), e (+x), s (+z), w (-x)."""
    a = math.degrees(math.atan2(x - center[0], z - center[1])) % 360
    return "senw"[int((a + 45) % 360 // 90)]


def label(p, cells, extent, center):
    """The floor under a touch: the nearest cell top at or just below the feet, its box being wider
    than a point. None off the map."""
    best = None
    for dx in (-0.3, 0, 0.3):
        for dz in (-0.3, 0, 0.3):
            for c in cells.get((math.floor(p[0] + dx), math.floor(p[2] + dz)), ()):
                d = abs(c["y"] - p[1])
                if d < 0.01 and (best is None or c["y"] > best["y"]):
                    best = c
    if best is None:
        return "?"
    e = extent[best["c"]]
    wide = max(e[1] - e[0], e[3] - e[2]) > ONE_PLACE
    return f'{best["c"]}{sector(p[0], p[2], center) if wide else ""}'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("floors")
    ap.add_argument("segments", nargs="+")
    ap.add_argument("--center", nargs=2, type=float, default=(-26.5, 219.5))
    ap.add_argument("--gate", type=int, default=2)
    ap.add_argument("--json")
    args = ap.parse_args()
    cells, extent = load_floors(args.floors)
    classes = collections.defaultdict(list)
    out = []
    for path in args.segments:
        for line in gzip.open(path, "rt"):
            a = json.loads(line)
            xyz, ground, e = a["xyz"], a["ground"], a["exit"]
            end = e if e is not None else len(xyz)
            # only what's flown from the reset
            if not xyz or math.dist(xyz[0], (-26.5, 282, 210.5)) > 1.5:
                continue
            touches = [i for i in range(1, end) if ground[i] and not ground[i - 1]]
            labels = [label(xyz[i], cells, extent, args.center) for i in touches]
            merged = [x for i, x in enumerate(labels) if i == 0 or x != labels[i - 1]]
            peak = max((p[1] for p in xyz[:end]), default=0)
            g = a["gates"][args.gate] if e is not None and a["gates"] else None
            rec = {
                "id": a["id"],
                "player": a["player"],
                "local": a["local"],
                "class": " ".join(merged) or "-",
                "touches": len(touches),
                "peak": peak,
                "exit_speed": a["exit_speed"],
                "gate": g,
                "finish": a["t"][a["finish"]] - a["t"][e] if a["finish"] is not None and e is not None else None,
            }
            classes[rec["class"]].append(rec)
            out.append(rec)
    rows = []
    for c, v in classes.items():
        out_of = [r for r in v if r["exit_speed"] is not None]
        gates = [r["gate"] for r in v if r["gate"] is not None and r["local"]]
        fin = [r["finish"] for r in v if r["finish"] is not None]
        rows.append(
            (
                min(gates) if gates else math.inf,
                c,
                len(v),
                len(out_of),
                statistics.median(r["touches"] for r in v),
                max(r["peak"] for r in v),
                max((r["exit_speed"] for r in out_of if r["local"]), default=None),
                min(gates) if gates else None,
                min(fin) if fin else None,
            )
        )
    print("| class | attempts | out | touches | peak y | best local exit | best local to gate | best finish |")
    print("|---|---|---|---|---|---|---|---|")
    for _, c, n, o, t, pk, sp, g, f in sorted(rows, key=lambda r: (r[0], -r[2])):
        if n < 2 and f is None and g is None:
            continue
        fmt = lambda x, d=2: "-" if x is None else f"{x:.{d}f}"
        print(f"| {c} | {n} | {o} | {t:g} | {pk:.1f} | {fmt(sp)} | {fmt(g)} | {fmt(f, 0)} |")
    print(f"\n{len(classes)} classes, {len(out)} attempts from the reset")
    if args.json:
        json.dump(out, open(args.json, "w"))


if __name__ == "__main__":
    main()
