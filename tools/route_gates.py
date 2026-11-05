#!/usr/bin/env python3
"""Gates across a course and the recorded ways between them, for the course's `routes` bin.

Run from the course directory:

    python3 ../../../tools/route_gates.py runs/routes/SET.json

A gate set below is a list of planes across the reference flight, the first where the timer starts.
Per gate this writes a table's nodes: three per axis, the middle one at the reference flight's crossing,
the ends past what the flights and fast attempts do there. why the middle at the reference? a table
reads exact at its nodes, so the flight we know is good is priced right. Per stretch between gates it
writes the flown paths, one per route: paths within `SAME_ROUTE` blocks of each other everywhere count
as one, and the fastest speaks for it.

Written for academy; the sets are its own.
"""
import glob
import gzip
import json
import math
import os
import sys

REFERENCE = "runs/relax/fullrange2.traj.jsonl"
SEGMENTS = ["runs/segments.jsonl.gz"] + sys.argv[2:]
# (name, axis, sign, plane coordinate, window half-width). Free cross sections are from the
# reference flight, 2026-11-05: blocks^2 of open plane around it.
EXIT = ("exit", 2, -1, 205.0, 14)
SETS = {
    # where everything goes through one hole
    "tight": [
        EXIT,
        ("t121", 0, 1, -2.0, 6),  # 14
        ("t211", 0, 1, 52.1, 6),  # 21
        ("t295", 0, -1, 47.4, 7),  # 44
        ("t361", 2, -1, -121.0, 7),  # 43
        ("t397", 0, 1, 8.3, 6),  # 23
        ("t505", 0, -1, 37.7, 7),  # 62
    ],
    # the middle of halls and drops
    "open": [
        EXIT,
        ("t61", 2, -1, 128.0, 30),  # 2228
        ("t139", 1, -1, 152.0, 25),  # 916
        ("t247", 2, -1, -34.0, 14),  # 384
        ("t283", 1, -1, 76.0, 18),  # 811
        ("t355", 1, -1, 33.0, 12),  # 439
        ("t421", 2, -1, -162.0, 9),  # 132
        ("t487", 1, -1, -21.0, 18),  # 641
    ],
}
SAME_ROUTE = 4.0
MAX_ROUTES = 12
RESAMPLE = 30
FAST_SHARE = 0.3
# how far the end nodes stand from the middle, at least: blocks, blocks/tick
MIN_POS = 0.8
MIN_VEL = 0.15
TAUS = [0.1, 0.5, 0.9]


def crossings(xyz, gates):
    """(first index past each gate, in order, or None from the first it misses)."""
    out = []
    i = 1
    for _, axis, sign, at, half in gates:
        found = None
        ref = gates_ref[(axis, sign, at)]
        free = [a for a in range(3) if a != axis]
        while i < len(xyz):
            p, q = xyz[i - 1], xyz[i]
            if sign * p[axis] <= sign * at < sign * q[axis] and all(abs(q[a] - ref[a]) <= half for a in free):
                found = i
                break
            i += 1
        if found is None:
            break
        out.append(found)
    return out


def read_flight(path):
    rows = [json.loads(line) for line in open(path).read().splitlines()[3:]]
    return [r["state"]["position"] for r in rows], [r["state"]["delta_movement"] for r in rows]


def resample(path, n=RESAMPLE):
    d = [0.0]
    for a, b in zip(path, path[1:]):
        d.append(d[-1] + math.dist(a, b))
    out = []
    j = 0
    for k in range(n):
        s = d[-1] * k / (n - 1)
        while j + 2 < len(d) and d[j + 1] < s:
            j += 1
        w = 0 if d[j + 1] == d[j] else (s - d[j]) / (d[j + 1] - d[j])
        out.append([path[j][m] + w * (path[j + 1][m] - path[j][m]) for m in range(3)])
    return out


def apart(a, b):
    return max(math.dist(p, q) for p, q in zip(a, b))


def quantile(v, q):
    v = sorted(v)
    return v[min(len(v) - 1, int(q * len(v)))]


def main():
    global gates_ref
    out_path = sys.argv[1]
    name = os.path.basename(out_path).removesuffix(".json")
    gates = SETS[name]
    xyz_r, v_r = read_flight(REFERENCE)
    # where the reference crosses: the plane alone, in order. why not its window? that's what's found here.
    gates_ref = {}
    i = 1
    for _, axis, sign, at, _h in gates:
        while not (sign * xyz_r[i - 1][axis] <= sign * at < sign * xyz_r[i][axis]):
            i += 1
        gates_ref[(axis, sign, at)] = xyz_r[i]
    # flights first: exact, and the ones to beat
    flights = []
    for p in sorted(glob.glob("runs/relax/*.traj.jsonl") + glob.glob("runs/optimize/*.traj.jsonl")):
        xyz, v = read_flight(p)
        flights.append({"label": "flight:" + os.path.basename(p).split(".traj")[0], "xyz": xyz, "v": v, "t": list(range(len(xyz)))})
    attempts = []
    for path in SEGMENTS:
        for line in gzip.open(path, "rt"):
            a = json.loads(line)
            if a["exit"] is None or len(a["xyz"]) < 30:
                continue
            attempts.append({"label": f'{a["player"]}:{a["id"].split(":")[-1]}', "xyz": a["xyz"], "t": a["t"]})
    # ---- crossings
    per_gate = [[] for _ in gates]
    per_seg = [[] for _ in gates]
    finish_z = -99.7
    for f, flight in [(True, x) for x in flights] + [(False, x) for x in attempts]:
        xyz, t = flight["xyz"], flight["t"]
        cross = crossings(xyz, gates)
        for g, c in enumerate(cross):
            k = min(c + 1, len(xyz) - 1)
            dt = max(1, t[k] - t[c - 1])
            v = [(xyz[k][m] - xyz[c - 1][m]) / dt for m in range(3)] if not f else flight["v"][c]
            frac = 0.5
            per_gate[g].append({"fast_key": t[c] - t[cross[0]], "flight": f, "p": xyz[c], "v": v})
        for g in range(len(cross)):
            a = cross[g]
            if g + 1 < len(cross):
                b = cross[g + 1]
            elif g + 1 == len(gates):
                b = next((i for i in range(a, len(xyz)) if xyz[i - 1][2] <= finish_z < xyz[i][2]), None)
                if b is None:
                    continue
            else:
                continue
            per_seg[g].append((t[b] - t[a], f, flight["label"], xyz[max(a - 1, 0) : b + 2]))
    # ---- nodes
    out_g = []
    _i = 1
    for g, (gname, axis, sign, at, half) in enumerate(gates):
        free = [a for a in range(3) if a != axis]
        ref_p = gates_ref[(axis, sign, at)]
        # the reference's own crossing, back on the plane
        j = next(c for c in range(_i, len(xyz_r)) if sign * xyz_r[c - 1][axis] <= sign * at < sign * xyz_r[c][axis])
        _i = j
        ref_v = v_r[j]
        rate = sign * ref_v[axis]
        tau = (sign * xyz_r[j][axis] - sign * at) / rate
        mid = [xyz_r[j][m] - ref_v[m] * tau for m in free] + list(ref_v)
        fast = sorted(per_gate[g], key=lambda c: c["fast_key"])
        fast = fast[: max(10, int(len(fast) * FAST_SHARE))]
        samples = [[c["p"][m] for m in free] + list(c["v"]) for c in fast]
        nodes = []
        for k in range(5):
            vals = [s[k] for s in samples]
            lo, hi = quantile(vals, 0.1), quantile(vals, 0.9)
            pad = MIN_POS if k < 2 else MIN_VEL
            nodes.append([min(lo, mid[k] - pad), mid[k], max(hi, mid[k] + pad)])
        nodes.append(TAUS)
        out_g.append(
            {
                "name": gname,
                "axis": axis,
                "sign": sign,
                "at": sign * at,
                "lo": [ref_p[m] - half for m in free],
                "hi": [ref_p[m] + half for m in free],
                "nodes": nodes,
                "reference": mid + [tau],
                "ref_tick": j + 1,
                "crossings": len(per_gate[g]),
            }
        )
    # ---- routes
    out_s = []
    for g, cands in enumerate(per_seg):
        cands = sorted(cands, key=lambda c: (c[0], not c[1]))
        routes = []
        for dt, f, label, path in cands:
            if len(path) < 3:
                continue
            r = resample(path)
            hit = next((x for x in routes if apart(x["_r"], r) < SAME_ROUTE), None)
            if hit:
                hit["n"] += 1
            else:
                routes.append({"label": label, "ticks": dt, "n": 1, "xyz": path, "_r": r})
        routes.sort(key=lambda r: r["ticks"])
        for r in routes:
            del r["_r"]
        out_s.append({"from": gates[g][0], "to": gates[g + 1][0] if g + 1 < len(gates) else "finish", "routes": routes[:MAX_ROUTES], "found": len(routes)})
    # the flight to start from: the reference at its exit
    os.makedirs(os.path.dirname(out_path) or ".", exist_ok=True)
    json.dump({"name": name, "gates": out_g, "segments": out_s}, open(out_path, "w"))
    for g in out_g:
        print(g["name"], "tick", g["ref_tick"], "crossings", g["crossings"], "nodes", [[round(v, 2) for v in n] for n in g["nodes"]])
    for s in out_s:
        print(s["from"], "->", s["to"], "routes", s["found"], [(r["label"][:18], r["ticks"], r["n"]) for r in s["routes"][:6]])


main()
