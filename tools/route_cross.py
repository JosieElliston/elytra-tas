#!/usr/bin/env python3
"""Crossing polished flights where they are in nearly the same state, for `optimize --hybrid`.

Every flight is placed along a ruler flight's path, as ticks ahead of the ruler at each point. A new flight
may change from one flight to another at a point where both have about the same position and velocity, and a
dynamic program finds the changes that end furthest ahead. why not `route_splice.py`'s fastest stretch between
gates? a stretch's time isn't its own: on academy, 2026-11-06, the fastest stretch t397-t505 was 3.5 ticks up
on speed its flight had paid for earlier, and every splice of it at the gate ended inside a wall. At matching
states the rest of the course is the same problem, so the leads add.

Run from the course directory:

    python3 ../../../tools/route_cross.py RULER.traj.jsonl OUT_PREFIX N FLIGHT.traj.jsonl... [--start NAME_SUBSTRING]...

Writes `runs/hybrid/OUT_PREFIX_K.traj.jsonl`, K < N, and prints each one's forecast and changes. A forecast is
a bound on what `optimize` keeps, not a promise. Flights must leave the start area by z < EXIT_Z; `--start`
limits which may be the first, whose prefix the new flight keeps.
"""
import json
import os
import random
import sys

import numpy as np

EXIT_Z = 205.0
FINISH_Z = -100.0
# Blocks and blocks per tick a change may be off by, tightest first: each gets a share of the N flights.
# why this tight? at 0.3 and 0.04 the forecast doubles, on changes every few ticks that collect the mismatch.
TOLERANCES = [(0.08, 0.01), (0.15, 0.02)]
# Ticks a change has to win by. why? a change costs the solver something at the join, however close.
CHANGE_COST = 0.15
# Each try after a tolerance's first leaves out this share of flights, for another set of changes.
LEAVE_OUT = 0.3
# Flights this far behind the ruler anywhere are off its route, and placing them says nothing.
OFF_ROUTE = 6.0


def read(path):
    lines = open(path).read().splitlines()
    head = [x for x in lines if "state" not in x or '"t"' not in x]
    rows = [json.loads(x) for x in lines if "state" in x and '"t"' in x]
    return head, rows


def door(pos):
    return next(i for i, p in enumerate(pos) if p[2] < EXIT_Z)


def place(q, ruler):
    """Each pose's continuous tick on the ruler, walking along it, and its distance off it."""
    s, off, j = [], [], 0
    for x in q:
        best = (1e9, float(j))
        for k in range(max(0, j - 3), min(len(ruler) - 1, j + 8)):
            a, d = ruler[k], ruler[k + 1] - ruler[k]
            dd = d @ d
            u = float(np.clip((x - a) @ d / dd, 0, 1)) if dd > 0 else 0.0
            e = float(np.linalg.norm(x - a - u * d))
            if e < best[0]:
                best = (e, k + u)
        s.append(best[1])
        off.append(best[0])
        j = int(best[1])
    return np.array(s), np.array(off)


def load(path, ruler, grid):
    head, rows = read(path)
    pos = np.array([r["state"]["position"] for r in rows])
    vel = np.array([r["state"]["delta_movement"] for r in rows])
    a = door(pos)
    s, off = place(pos[a:], ruler)
    if s[-1] < grid[-1] or off.max() > OFF_ROUTE:
        return None
    t = np.arange(len(s), dtype=float)
    ok = np.concatenate([[True], np.diff(s) > 0])
    interp = lambda y: np.interp(grid, s[ok], y[ok])
    return dict(
        name=os.path.basename(path).split(".traj")[0],
        head=head,
        rows=rows,
        door=a,
        s=s,
        t=interp(t),
        p=np.stack([interp(pos[a:, i]) for i in range(3)], 1),
        v=np.stack([interp(vel[a:, i]) for i in range(3)], 1),
    )


def solve(flights, allowed_first, tol):
    """The best last lead and its changes, as (grid index, flight) from the start."""
    n, m = len(flights), len(flights[0]["t"])
    lead = np.array([f["grid"] - f["t"] for f in flights])
    p = np.array([f["p"] for f in flights])
    v = np.array([f["v"] for f in flights])
    best = np.where(allowed_first, lead[:, 0], -np.inf)
    back = np.zeros((m, n), dtype=np.int32)
    back[0] = np.arange(n)
    for i in range(1, m):
        gain = lead[:, i] - lead[:, i - 1]
        same = np.linalg.norm(p[:, i, None] - p[None, :, i], axis=2) < tol[0]
        same &= np.linalg.norm(v[:, i, None] - v[None, :, i], axis=2) < tol[1]
        # the value of changing from g to f at this tick: f and g both index the rows' flights
        score = np.where(same, best[:, None] - CHANGE_COST, -np.inf)
        np.fill_diagonal(score, best)
        g = score.argmax(axis=0)
        best = score[g, np.arange(n)] + gain
        back[i] = g
    f = int(best.argmax())
    value = float(best[f])
    path = []
    for i in range(m - 1, 0, -1):
        g = int(back[i, f])
        if g != f:
            path.append((i, f))
        f = g
    path.append((0, f))
    return value, path[::-1]


def write(flights, grid, path, name):
    base = flights[path[0][1]]
    rows = list(base["rows"][: base["door"]])
    for k, (i, f) in enumerate(path):
        fl = flights[f]
        lo = grid[i]
        hi = grid[path[k + 1][0]] if k + 1 < len(path) else np.inf
        rows += [fl["rows"][fl["door"] + j] for j in range(len(fl["s"])) if lo <= fl["s"][j] < hi or (hi == np.inf and fl["s"][j] >= lo)]
    last = flights[path[-1][1]]
    rows += last["rows"][last["door"] + len(last["s"]) :]
    out = []
    for t, r in enumerate(rows):
        r = dict(r)
        r["t"] = rows[0]["t"] + t
        out.append(json.dumps(r))
    with open(f"runs/hybrid/{name}.traj.jsonl", "w") as w:
        w.write("\n".join(base["head"] + out) + "\n")


def main():
    args = sys.argv[1:]
    starts = []
    while "--start" in args:
        k = args.index("--start")
        starts.append(args[k + 1])
        del args[k : k + 2]
    _, ruler_rows = read(args[0])
    prefix, count = args[1], int(args[2])
    pos = np.array([r["state"]["position"] for r in ruler_rows])
    a = door(pos)
    fin = next(i for i in range(a + 1, len(pos)) if pos[i - 1][2] <= FINISH_Z < pos[i][2])
    ruler = pos[a:]
    grid = np.arange(0.0, fin - a - 2.0)
    flights = []
    for path in args[3:]:
        f = load(path, ruler, grid)
        if f:
            f["grid"] = grid
            flights.append(f)
    print(len(flights), "of", len(args) - 3, "flights on the ruler's route, ruler", fin - a, "ticks door to finish")
    first = np.array([not starts or any(s in f["name"] for s in starts) for f in flights])
    alone = [(float(grid[-1] - f["t"][-1]), f["name"]) for f in flights if first[flights.index(f)]]
    print("best alone:", [(round(v, 2), n) for v, n in sorted(alone, reverse=True)[:4]])
    random.seed(1)
    seen, k = set(), 0
    for ti, tol in enumerate(TOLERANCES):
        want = (count * (ti + 1)) // len(TOLERANCES)
        for attempt in range(40 * count):
            if k >= want:
                break
            sub = list(range(len(flights)))
            if attempt:
                sub = [i for i in sub if random.random() > LEAVE_OUT]
            fs = [flights[i] for i in sub]
            value, path = solve(fs, first[sub], tol)
            path = [(i, sub[f]) for i, f in path]
            key = tuple(flights[f]["name"] for _, f in path)
            if key in seen:
                continue
            seen.add(key)
            write(flights, grid, path, f"{prefix}_{k}")
            print(
                f"{prefix}_{k}: tol {tol}, forecast {value:.2f} ahead: "
                + " ".join(f"{flights[f]['name']}@{grid[i]:.0f}" for i, f in path)
            )
            k += 1


main()
