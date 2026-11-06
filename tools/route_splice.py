#!/usr/bin/env python3
"""Crossing polished flights at gates: per stretch the flights' ticks, then new flights taking each stretch
from whichever did it best, for `optimize --hybrid`. why? a stitched flight polishes in minutes, and a
route's stretches are close to independent at a tight gate, so the best of each is worth trying first.

Run from the course directory:

    python3 ../../../tools/route_splice.py SET.json OUT_PREFIX N|RECIPES.json FLIGHT.traj.jsonl...

Writes `runs/hybrid/OUT_PREFIX_K.traj.jsonl`, K < N: the best stretch of each, then with one or two
stretches swapped for their next best. Or a file's choices: a list, per new flight, of the flight's name
to take for each stretch. Ranks by the tick each flight crosses, between rows. Flights ought to hold the same constraints, or the result
inherits the laxest stretch's; `optimize --server` then has to repair it.
"""
import itertools
import json
import os
import sys

FINISH_Z = -100.0


def read(path):
    lines = open(path).read().splitlines()
    return lines[:3], [json.loads(x) for x in lines[3:]]


def fractions(rows, cut, gates):
    """Each cut's tick between rows: why? a stretch's gain is often under a tick, and row counts round it away."""
    p = [r["state"]["position"] for r in rows]
    out = []
    for i, g in zip(cut, [(x["axis"], x["at"], x["sign"]) for x in gates] + [(2, FINISH_Z, 1)]):
        a, at, s = g
        u, v = s * p[i - 1][a], s * p[i][a]
        out.append(i - 1 + (at - u) / (v - u))
    return out


def cuts(rows, gates):
    """The row index past each gate and the finish, or None if it misses one."""
    p = [r["state"]["position"] for r in rows]
    out = []
    i = 1
    for g in gates:
        a, s, at = g["axis"], g["sign"], g["at"]
        f = [m for m in range(3) if m != a]
        while i < len(p) and not (
            s * p[i - 1][a] <= at < s * p[i][a]
            and all(g["lo"][k] <= p[i][f[k]] <= g["hi"][k] for k in range(2))
        ):
            i += 1
        if i >= len(p):
            return None
        out.append(i)
    while i < len(p) and not (p[i - 1][2] <= FINISH_Z < p[i][2]):
        i += 1
    if i >= len(p):
        return None
    out.append(i)
    return out


def main():
    gates = json.load(open(sys.argv[1]))["gates"]
    prefix = sys.argv[2]
    recipes = json.load(open(sys.argv[3])) if sys.argv[3].endswith(".json") else None
    count = len(recipes) if recipes else int(sys.argv[3])
    flights = []
    for path in sys.argv[4:]:
        head, rows = read(path)
        c = cuts(rows, gates)
        if c:
            flights.append((os.path.basename(path).split(".traj")[0], head, rows, c, fractions(rows, c, gates)))
    n = len(gates)
    took = lambda f, s: flights[f][4][s + 1] - flights[f][4][s]
    per = [sorted(range(len(flights)), key=lambda f: took(f, s)) for s in range(n)]
    for s in range(n):
        names = [(flights[f][0], round(took(f, s), 2)) for f in per[s][:6]]
        print(gates[s]["name"], names)
    print("best of each sums to", round(sum(took(per[s][0], s) for s in range(n)), 2), "over", len(flights), "flights")

    def distinct_next(s):
        """Next best flight on stretch s with a different time, so a swap isn't a tie's twin."""
        for f in per[s][1:]:
            if took(f, s) > took(per[s][0], s) + 0.02:
                return f
        return per[s][1] if len(per[s]) > 1 else per[s][0]

    choices = [tuple(per[s][0] for s in range(n))]
    # ties first: the same ticks by another flight is another route
    for s in range(n):
        for f in per[s][1:3]:
            c = list(choices[0])
            c[s] = f
            choices.append(tuple(c))
    for a, b in itertools.combinations(range(n), 2):
        c = list(choices[0])
        c[a], c[b] = distinct_next(a), distinct_next(b)
        choices.append(tuple(c))
    if recipes:
        at = {f[0]: i for i, f in enumerate(flights)}
        choices = [tuple(at[x] for x in r) for r in recipes]
    seen = []
    for c in choices:
        if c not in seen:
            seen.append(c)
    for k, c in enumerate(seen[:count]):
        base = flights[c[0]]
        rows = list(base[2][: base[3][0]])
        for s in range(n):
            f = flights[c[s]]
            rows += f[2][f[3][s] : f[3][s + 1]]
        tail = flights[c[-1]]
        rows += tail[2][tail[3][-1] : tail[3][-1] + 6]
        out = []
        for t, r in enumerate(rows):
            r = dict(r)
            r["t"] = rows[0]["t"] + t
            out.append(r)
        with open(f"runs/hybrid/{prefix}_{k}.traj.jsonl", "w") as w:
            w.write("\n".join(base[1] + [json.dumps(r) for r in out]) + "\n")
        print(f"{prefix}_{k}", [flights[f][0] for f in c], "sum", round(sum(took(c[s], s) for s in range(n)), 2))


main()
