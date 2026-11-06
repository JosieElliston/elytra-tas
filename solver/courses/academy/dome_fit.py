#!/usr/bin/env python3
"""The start room's exit value, from the `dome` bin's samples: a quadratic fit of what a way through the
door is worth, baked onto a grid as `runs/dome/table.json` for the bin's `starts` phase.

why a fit? a sample is whole ticks and a few of them are a poor solve, which a table read directly
would fly straight at. Trimmed once, since a poor solve only ever reads too slow.

usage (from the course directory): python3 dome_fit.py SAMPLES.jsonl...
"""
import json
import sys

import numpy as np

AXES = ["x", "y", "vx", "vy", "vz"]
GRID = [5, 7, 5, 9, 9]
# Residuals above this many sigma are poor solves; below, a lucky one is still a real flight.
TRIM_SIGMA = 2.0


def features(c):
    """Quadratic in `c`, already centered and scaled."""
    n = c.shape[1]
    cols = [np.ones(len(c))] + [c[:, i] for i in range(n)]
    cols += [c[:, i] * c[:, j] for i in range(n) for j in range(i, n)]
    return np.stack(cols, axis=1)


def main():
    rows = [json.loads(line) for path in sys.argv[1:] for line in open(path)]
    c = np.array([r["c"] for r in rows])
    v = np.array([r["value"] for r in rows])
    lo, hi = c.min(axis=0), c.max(axis=0)
    mid, half = (lo + hi) / 2, (hi - lo) / 2
    scaled = (c - mid) / half
    keep = np.ones(len(v), bool)
    for _ in range(3):
        beta, *_ = np.linalg.lstsq(features(scaled[keep]), v[keep], rcond=None)
        res = v - features(scaled) @ beta
        sigma = res[keep].std()
        keep = res < TRIM_SIGMA * sigma
    print(f"{len(v)} samples, kept {keep.sum()}, residual {sigma:.2f} ticks, values {v.min():.1f} .. {v.max():.1f}")
    # what a step across each axis is worth at the middle of the box
    print("per unit at the box's middle:", {a: round(beta[1 + i] / half[i], 2) for i, a in enumerate(AXES)})
    nodes = [np.linspace(lo[i], hi[i], GRID[i]) for i in range(5)]
    mesh = np.meshgrid(*nodes, indexing="ij")
    g = (np.stack([m.ravel() for m in mesh], axis=1) - mid) / half
    values = features(g) @ beta
    table = {"nodes": [n.tolist() for n in nodes], "values": values.tolist()}
    json.dump(table, open("runs/dome/table.json", "w"))
    best = g[np.argmin(values)] * half + mid
    print("grid least", round(float(values.min()), 1), "at", dict(zip(AXES, best.round(2).tolist())))


main()
