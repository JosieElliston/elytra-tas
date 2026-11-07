#!/usr/bin/env python3
"""Where `optimize --chain` plans for one run come apart: position against a reference line, by arclength.

python3 diverge.py NAME [REF]; REF defaults to L_round5. Reads runs/secant/chain/NAME_chain_*.tsv.
"""
import glob, re, sys, bisect
import numpy as np

name = sys.argv[1]
ref_name = sys.argv[2] if len(sys.argv) > 2 else "L_round5"
pre = f"runs/secant/chain/{name}_chain_"
plans = {f[len(pre):-4]: np.loadtxt(f, usecols=(2, 3, 4)) for f in glob.glob(pre + "*.tsv")}
ref = plans[ref_name]
seg = np.diff(ref, axis=0)
sl = np.linalg.norm(seg, axis=1)
S = np.r_[0, np.cumsum(sl)]

def project(p):
    """Progress along the reference and distance from it, per tick, monotone within a window."""
    i, out = 0, []
    for q in p:
        lo, hi = max(0, i - 3), min(len(seg), i + 40)
        d = q - ref[lo:hi]
        t = np.clip((d * seg[lo:hi]).sum(1) / (sl[lo:hi] ** 2 + 1e-12), 0, 1)
        e = np.linalg.norm(d - t[:, None] * seg[lo:hi], axis=1)
        k = int(np.argmin(e))
        i = lo + k
        out.append((S[i] + t[k] * sl[i], e[k]))
    return np.array(out)

def tick_at(pr, grid):
    return np.interp(grid, np.maximum.accumulate(pr[:, 0]), np.arange(len(pr)))

proj = {k: project(v) for k, v in plans.items()}
grid = np.arange(0, S[-1], S[-1] / 24)
print(f"reference {ref_name}: {len(ref)} ticks, {S[-1]:.0f} blocks")
print("where along it (progress: x y z):")
for g in grid:
    k = bisect.bisect(S, g) - 1
    print(f"  {g:6.0f}: {ref[k][0]:7.1f} {ref[k][1]:6.1f} {ref[k][2]:7.1f}")

def key(k):
    m = re.match(r"(.*?)_(round|backround)(\d+)$", k)
    return (m[1], m[2] == "backround", int(m[3])) if m else None

print("\nper round, against the reference: ticks | largest distance from the reference, and the first progress past 1 and past 3 blocks")
for k in sorted((k for k in plans if key(k)), key=key):
    pr = proj[k]
    first = lambda d: next((f"{a:.0f}" for a, e in pr if e > d), "-")
    print(f"  {k:24} {len(pr):4d} | max {pr[:,1].max():6.2f}  >1 at {first(1):>5}  >3 at {first(3):>5}")

print("\nticks behind the reference at each progress (negative is ahead)")
# the last round of each stretch; `*_back.tsv` files are shared by every arm, so they aren't read
def last(prefix):
    r = sorted((k for k in plans if k.startswith(prefix)), key=key)
    return r[-1] if r else None
fin = [k for k in [last(f"{m}_{w}") for m in ["L", "Lo1", "Lm.1", "xz", "xyz", "all"] for w in ["round", "backround"]] if k]
print("  progress " + "".join(f"{re.sub('backround.*','bk',re.sub('_round.*','_ex',k)):>9}" for k in fin))
g0 = tick_at(proj[ref_name], grid)
for j, g in enumerate(grid):
    print(f"  {g:8.0f} " + "".join(f"{tick_at(proj[k], np.array([g]))[0] - g0[j]:9.2f}" for k in fin))
print("  distance " + "".join(f"{proj[k][:,1].max():9.2f}" for k in fin))
