#!/usr/bin/env python3
"""Seeds for trying a course's other routes: a flight with one stretch swapped for a recorded
attempt's, one per route class, for `optimize --hybrid`.

A class is the sequence of free-space openings an attempt passes at some cross sections (from the
course's `section` bin), or for a stretch with no walls to split it, a band of x and y at one plane.

Written for academy; the stretches below are its own. Run from the course directory:

    python3 ../../../tools/route_hybrids.py runs/relax/NAME.traj.jsonl runs/hybrid
"""
import collections
import copy
import gzip
import json
import os
import subprocess
import sys
from collections import deque

SECTION = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../solver/target/release/section")
# the z = 178 and z = 100 walls of the hall after the start: (planes, x0, x1, y0, y1)
HOLES = ([178, 100], -60, -10, 170, 215)
HOLE_STRETCH = (190, 75)
# the candle room: bands at z = -18
BANDS = (-18, (40, 80, 95, 132), (2, 3))
BAND_STRETCH = (-3, -56)
TOP_BANDS = 10


def plane(z, x0, x1, y0, y1):
    """Each 0.1-block cell's free-space component at plane z."""
    out = subprocess.run([SECTION, *map(str, (z, x0, x1, y0, y1))], capture_output=True, text=True, check=True)
    rows = out.stdout.split()
    h, w = len(rows), len(rows[0])
    lab = [[0] * w for _ in range(h)]
    n = 0
    for j in range(h):
        for i in range(w):
            if rows[j][i] == "0" and not lab[j][i]:
                n += 1
                lab[j][i] = n
                q = deque([(j, i)])
                while q:
                    b, c = q.popleft()
                    for d, e in ((b + 1, c), (b - 1, c), (b, c + 1), (b, c - 1)):
                        if 0 <= d < h and 0 <= e < w and rows[d][e] == "0" and not lab[d][e]:
                            lab[d][e] = n
                            q.append((d, e))
    return lab


def crossing(xyz, z, box):
    """The index of the first move down through plane z inside box (x0, x1, y0, y1)."""
    for i in range(1, len(xyz)):
        p, q = xyz[i - 1], xyz[i]
        if p[2] >= z > q[2] and box[0] <= q[0] <= box[1] and box[2] <= q[1] <= box[3]:
            return i
    return None


def classes(attempts, planes, x0, x1, y0, y1):
    labels = {z: plane(z, x0, x1, y0, y1) for z in planes}
    out = collections.defaultdict(list)
    for a in attempts:
        sig, ticks = [], []
        for z in planes:
            i = crossing(a["xyz"], z, (x0, x1 - 0.01, y0, y1 - 0.01))
            if i is None:
                break
            p, q = a["xyz"][i - 1], a["xyz"][i]
            f = (p[2] - z) / (p[2] - q[2])
            x, y = p[0] + f * (q[0] - p[0]), p[1] + f * (q[1] - p[1])
            sig.append(labels[z][int((y - y0) * 10)][int((x - x0) * 10)])
            ticks.append(a["t"][i])
        else:
            out[tuple(sig)].append((ticks[-1] - ticks[0], a["id"]))
    return out


def hybrid(rows, head, a, z0, z1, box, path):
    """rows with its stretch from z0 to z1 replaced by a's, per tick. Remote attempts are sampled
    about every other tick, so this interpolates."""
    xyz = [r["state"]["position"] for r in rows]
    bi, bj = crossing(xyz, z0, box), crossing(xyz, z1, box)
    ai, aj = crossing(a["xyz"], z0, box), crossing(a["xyz"], z1, box)
    if None in (bi, bj, ai, aj):
        return False
    new = rows[:bi]
    for i in range(ai, aj):
        k = max(1, a["t"][i + 1] - a["t"][i])
        p, q = a["xyz"][i], a["xyz"][i + 1]
        turn = ((a["yaw"][i + 1] - a["yaw"][i] + 180) % 360) - 180
        for j in range(k):
            f = j / k
            r = copy.deepcopy(rows[bi])
            r["state"]["position"] = [p[m] + f * (q[m] - p[m]) for m in range(3)]
            r["state"]["delta_movement"] = [(q[m] - p[m]) / k for m in range(3)]
            r["state"]["fall_flying"] = True
            for flag in ("on_ground", "horizontal_collision", "vertical_collision"):
                r["state"][flag] = False
            r["input"]["yaw"] = a["yaw"][i] + f * turn
            r["input"]["pitch"] = a["pitch"][i] + f * (a["pitch"][i + 1] - a["pitch"][i])
            new.append(r)
    new += copy.deepcopy(rows[bj:])
    for t, r in enumerate(new):
        r["t"] = rows[0]["t"] + t
    with open(path, "w") as f:
        f.write("\n".join(head + [json.dumps(r) for r in new]) + "\n")
    return True


def main():
    base, out = sys.argv[1], sys.argv[2]
    os.makedirs(out, exist_ok=True)
    lines = open(base).read().splitlines()
    head, rows = lines[:3], [json.loads(x) for x in lines[3:]]
    attempts = [json.loads(x) for x in gzip.open("runs/segments.jsonl.gz", "rt")]
    by_id = {a["id"]: a for a in attempts}

    planes, x0, x1, y0, y1 = HOLES
    for k, v in sorted(classes(attempts, planes, x0, x1, y0, y1).items(), key=lambda kv: min(kv[1])):
        dt, best = min(v)
        name = f"start_{'_'.join(map(str, k))}_{best.split(':')[-2]}"
        ok = hybrid(rows, head, by_id[best], *HOLE_STRETCH, (x0, x1, y0, y1), f"{out}/{name}.traj.jsonl")
        print(name, len(v), "attempts, best", dt, "ticks", "" if ok else "(no splice)")

    z, box, (dx, dy) = BANDS
    bands = collections.defaultdict(list)
    for a in attempts:
        i, i0, i1 = (crossing(a["xyz"], zz, box) for zz in (z, *BAND_STRETCH))
        if None not in (i, i0, i1):
            p = a["xyz"][i]
            bands[(round(p[0] / dx) * dx, round(p[1] / dy) * dy)].append((a["t"][i1] - a["t"][i0], a["id"]))
    for k, v in sorted(bands.items(), key=lambda kv: min(kv[1]))[:TOP_BANDS]:
        dt, best = min(v)
        name = f"candles_{k[0]}_{k[1]}_{best.split(':')[-2]}"
        ok = hybrid(rows, head, by_id[best], *BAND_STRETCH, box, f"{out}/{name}.traj.jsonl")
        print(name, len(v), "attempts, best", dt, "ticks", "" if ok else "(no splice)")


if __name__ == "__main__":
    main()
