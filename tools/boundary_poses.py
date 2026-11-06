"""A flight's poses against the block grid at academy's timer start and finish, from the side and above: which pose is
the last on the near side of each region's face, which is the first past it, and which move fires the event.

`python3 tools/boundary_poses.py [FLIGHT]`, FLIGHT in `runs/relax` (g527 by default); needs `ideas` built. Writes
`runs/anneal/views/boundary_FLIGHT.png` and prints the poses.
"""
import json, math, re, subprocess, sys
from pathlib import Path
import matplotlib; matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import Rectangle
A = str(Path(__file__).resolve().parent.parent / "solver/courses/academy")
NAME = sys.argv[1] if len(sys.argv) > 1 else "g527"
rows = [json.loads(l) for l in open(f"{A}/runs/relax/{NAME}.traj.jsonl")][3:]
P = {r["t"]: r["state"]["position"] for r in rows}
V = {r["t"]: r["state"]["delta_movement"] for r in rows}
reg = json.load(open(f"{A}/course.json"))["regions"]
INK, MUTED, GRID, BLUE, ORANGE, AQUA, WALL = "#0b0b0b", "#52514e", "#d6d5cf", "#2a78d6", "#eb6834", "#1baf7a", "#8b8a84"

def boxes(lo, hi):
    out = subprocess.run([f"{A}/../../target/release/ideas", "blocks", *map(str, lo), *map(str, hi)], capture_output=True, text=True, cwd=A).stdout
    b = []
    for l in out.splitlines():
        for m in re.findall(r"\[(-?[\d.]+) (-?[\d.]+) (-?[\d.]+) \.\. (-?[\d.]+) (-?[\d.]+) (-?[\d.]+)\]", l):
            v = list(map(float, m)); b.append((v[:3], v[3:]))
    return b

def fire(ts, inside, entering):
    """first pose on the far side of the plane, and the first block-changing move from there"""
    first = next(t for t in ts if (inside(P[t]) if entering else not inside(P[t])))
    blk = lambda p: tuple(math.floor(v) for v in p)
    f = next(t for t in ts if t > first and blk(P[t - 1]) != blk(P[t]))
    return first, f

def panel(ax, ts, first, fired, axis, plane, band, shade, wallset, other, labels, flip):
    # axis 1 = y (side), 0 = x (top); horizontal is always z
    zs = [P[t][2] for t in ts]; vs = [P[t][axis] for t in ts]
    zl = (min(zs) - 0.9, max(zs) + 0.9); vl = (min(vs) - 0.9, max(vs) + 0.9)
    ax.axvspan(*shade(zl), color=AQUA, alpha=0.13, lw=0, zorder=0)
    for z in range(math.floor(zl[0]), math.ceil(zl[1]) + 1): ax.axvline(z, color=GRID, lw=0.8, zorder=1)
    for v in range(math.floor(vl[0]), math.ceil(vl[1]) + 1): ax.axhline(v, color=GRID, lw=0.8, zorder=1)
    for b in band: ax.axvline(b, color=AQUA, lw=1, ls=(0, (2, 3)), zorder=2)
    ax.axvline(plane, color=AQUA, lw=2, zorder=2)
    for lo, hi in wallset:
        o = [P[t][other] for t in ts]
        # the gliding box: 0.6 wide around x, 0.6 tall above the feet in y
        below, above = (min(o) + 1e-3, max(o) + 0.6) if other == 1 else (min(o) - 0.3, max(o) + 0.3)
        if hi[other] <= below or lo[other] >= above: continue
        ax.add_patch(Rectangle((lo[2], lo[axis]), hi[2] - lo[2], hi[axis] - lo[axis], color=WALL, alpha=0.55, lw=0, zorder=2))
    a, b = sorted((P[fired - 1][2], P[fired][2]))
    for z in range(math.ceil(a), math.floor(b) + 1): ax.axvline(z, color=ORANGE, lw=1.5, zorder=2)
    ax.plot(zs, vs, color=BLUE, lw=2, zorder=3, solid_capstyle="round")
    for t in ts:
        p = P[t]; hot = t == fired; key = t in labels
        ax.scatter(p[2], p[axis], s=70 if key else 36, color=ORANGE if hot else BLUE, edgecolor="white", linewidth=1.5, zorder=4)
        txt = f"t{t}" + (f"\n{labels[t]}" if key else "")
        ax.annotate(txt, (p[2], p[axis]), xytext=(0, 13), textcoords="offset points", ha="center", va="bottom", fontsize=8.5, color=INK if key else MUTED, zorder=5, linespacing=1.15, bbox=dict(boxstyle="round,pad=0.15", fc="#fcfcfb", ec="none", alpha=0.85))
    ax.set_xlim(*(zl[::-1] if flip else zl)); ax.set_ylim(*vl); ax.set_aspect("equal")
    ax.set_xlabel("z  (direction of travel →)", fontsize=9, color=MUTED); ax.set_ylabel("y" if axis == 1 else "x", fontsize=9, color=MUTED)
    ax.tick_params(colors=MUTED, labelsize=8, length=0)
    for s in ax.spines.values(): s.set_visible(False)
    ax.set_xticks(range(math.floor(zl[0]), math.ceil(zl[1]) + 1)); ax.set_yticks(range(math.floor(vl[0]), math.ceil(vl[1]) + 1))

ts_all = sorted(P)
s, f = reg["start_area"], reg["finish"]
in_s = lambda p: p[2] >= s["working"]["min"][2]
in_f = lambda p: p[2] >= f["working"]["min"][2]
door = next(t for t in ts_all if t > 10 and not in_s(P[t]))
s_first, s_fire = fire([t for t in ts_all if t >= door], in_s, False)
z0 = f["working"]["min"][2]
f_first = next(t for t in ts_all if t > s_fire and P[t - 1][2] < z0 <= P[t][2])
f_fire = next(t for t in ts_all if t > f_first and tuple(map(math.floor, P[t - 1])) != tuple(map(math.floor, P[t])))
start_ts = list(range(s_first - 2, s_fire + 2)); end_ts = list(range(f_first - 2, f_fire + 2))
wall_s = boxes([-31, 202, 198], [-25, 210, 208]); wall_e = boxes([-9, -59, -104], [-4, -54, -95])
L_s = {s_first - 1: "last inside", s_first: "first outside", s_fire: "timer fires"}
L_e = {f_first - 1: "last outside", f_first: "first inside", f_fire: "finish fires"}

fig, ax = plt.subplots(2, 2, figsize=(14, 9.4), facecolor="#fcfcfb")
sb = [s["inner"]["min"][2], s["outer"]["min"][2]]; fb = [f["inner"]["min"][2], f["outer"]["min"][2]]
shade_s = lambda zl: (s["working"]["min"][2], zl[1]); shade_f = lambda zl: (z0, zl[1])
panel(ax[0][0], start_ts, s_first, s_fire, 1, s["working"]["min"][2], sb, shade_s, wall_s, 0, L_s, True)
panel(ax[0][1], start_ts, s_first, s_fire, 0, s["working"]["min"][2], sb, shade_s, wall_s, 1, L_s, True)
panel(ax[1][0], end_ts, f_first, f_fire, 1, z0, fb, shade_f, wall_e, 0, L_e, False)
panel(ax[1][1], end_ts, f_first, f_fire, 0, z0, fb, shade_f, wall_e, 1, L_e, False)
for a, title in zip(ax.flat, ["Start, from the side", "Start, from above", "Finish, from the side", "Finish, from above"]):
    a.set_title(title, loc="left", fontsize=11, color=INK, fontweight="bold", pad=8)
from matplotlib.lines import Line2D
h = [Line2D([], [], color=BLUE, marker="o", lw=2, label="feet position, per tick"), Line2D([], [], color=ORANGE, marker="o", lw=0, label="pose whose move fires the event"),
     Rectangle((0, 0), 1, 1, color=AQUA, alpha=.2, label="region (working box)"), Line2D([], [], color=AQUA, lw=2, label="region's working face"),
     Line2D([], [], color=AQUA, lw=1, ls=(0, (2, 3)), label="fitted inner / outer face"), Rectangle((0, 0), 1, 1, color=WALL, alpha=.55, label="collision boxes"),
     Line2D([], [], color=GRID, lw=1, label="block boundaries"), Line2D([], [], color=ORANGE, lw=1.5, label="boundaries the firing move crosses")]
fig.legend(handles=h, loc="lower center", ncol=4, frameon=False, fontsize=9, labelcolor=MUTED)
def note(a, lines): a.text(0.015, 0.03, "\n".join(lines), transform=a.transAxes, fontsize=8.5, color=INK, va="bottom", ha="left", linespacing=1.35, zorder=6, bbox=dict(boxstyle="round,pad=0.35", fc="#fcfcfb", ec=GRID))
ps, pf = s["working"]["min"][2], z0
note(ax[0][0], [f"t{s_first-1} is {P[s_first-1][2]-ps:.3f} blocks inside the face", f"t{s_first} is {ps-P[s_first][2]:.3f} past it", f"t{s_first}'s move crosses z = 203, 202; no x or y"])
note(ax[1][0], [f"t{f_first-1} is {pf-P[f_first-1][2]:.3f} blocks short of the face", f"t{f_first} is {P[f_first][2]-pf:.3f} past it", f"t{f_first}'s move crosses z = -99 by {P[f_fire][2]+99:.3f}; no x or y"])
fig.suptitle(f"{NAME}: poses against the block grid at the timer's start and finish", x=0.04, ha="left", fontsize=13, color=INK, fontweight="bold")
fig.tight_layout(rect=(0, 0.07, 1, 0.97))
Path(f"{A}/runs/anneal/views").mkdir(parents=True, exist_ok=True)
fig.savefig(f"{A}/runs/anneal/views/boundary_{NAME}.png", dpi=130)

print("start"); 
for t in start_ts: print(t, [round(v, 3) for v in P[t]], "block", [math.floor(v) for v in P[t]], "v", [round(v, 3) for v in V[t]], L_s.get(t, ""))
print("finish")
for t in end_ts: print(t, [round(v, 3) for v in P[t]], "block", [math.floor(v) for v in P[t]], "v", [round(v, 3) for v in V[t]], L_e.get(t, ""))
