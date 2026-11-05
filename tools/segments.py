"""Every attempt of every player a course's replays saw, timed against a reference route.

usage: python3 tools/segments.py COURSE_DIR REPLAY... [-o OUTDIR] [--gate-every N] [--tapes]

COURSE_DIR holds course.json; its `start.source` tape is the reference route. OUTDIR
defaults to COURSE_DIR/runs. Writes:

    segments.jsonl.gz   one attempt per line (below)
    splits.md           the exits of the start area, then the best stretch between gates
    seeds/*.tas         with --tapes: the local players' finished runs, as tapes

Attempt: {"id", "source", "player", "local", "first_tick", "exit": index or null,
"exit_speed", "exit_v": [vx, vy, vz], "finish": index or null, "reached": gate index or
null, "gates": [time or null, ...], "t": [ticks], "xyz": [[x, y, z]...], "yaw": [...],
"pitch": [...], "ground": [0|1...], "glide": [0|1...]}. Indices are into those lists.
`gates` are ticks since the timer started (or, with no exit, since the attempt's first
sample), interpolated along the route between samples.

An attempt runs between teleports (a jump over TELEPORT_SPEED blocks/tick), or for a
remote player also gaps of over MAX_GAP ticks. What the poses are depends on the player
(players.py): local ones are exact; remote ones come every 2 ticks or so, on the 1/4096
grid, with rotation in 1.4 degree steps, and their ticks are when the client got them.
So a remote time is good to a tick or so and its rotation only says roughly how it flew:
a remote attempt seeds a route, not a tape.

The route is the tape's poses. A sample's progress is its projection on the route, found
forward from the last one, so a course passing under itself doesn't jump. A sample over
ROUTE_RADIUS away isn't on the route; a route that leaves for good ends the attempt's
timing, though the samples stay in the file.
"""

from __future__ import annotations

import argparse
import gzip
import json
import math
import struct
import sys
from dataclasses import dataclass
from pathlib import Path

import extract_run as er
import flashback as fb
import players

TELEPORT_SPEED = 8.0  # blocks/tick; elytra speed tops out near 4, boosters aside
MAX_GAP = 40  # ticks without a remote pose (out of tracking range)
ROUTE_RADIUS = 12.0  # blocks from the route
ROUTE_WINDOW = 60  # route ticks a sample's progress may jump ahead
OFF_ROUTE_END = 8  # samples off the route in a row that end the timing
GATE_GAP = 4  # ticks; the widest gap between samples a remote gate time may interpolate over
V_SPAN = 4  # ticks a remote velocity is measured over. why? two of its 2-tick moves can reach the client in one tick
MIN_SAMPLES = 10


def in_box(p, box) -> bool:
    return all(box["min"][k] <= p[k] <= box["max"][k] for k in range(3))


@dataclass
class Route:
    points: list[tuple[float, float, float]]
    exit: int  # first point outside the start area
    finish: int  # first point in the finish

    @classmethod
    def from_tape(cls, path: Path, course: dict) -> "Route":
        start, finish = course["regions"]["start_area"]["working"], course["regions"]["finish"]["working"]
        pts = []
        for line in path.read_text().splitlines():
            f = line.split("#", 1)[0].split()
            if len(f) == 7 and f[0].isdigit():
                pts.append(tuple(struct.unpack(">d", bytes.fromhex(h))[0] for h in f[4:7]))
        i = next(i for i, p in enumerate(pts) if not in_box(p, start))
        j = next(j for j, p in enumerate(pts) if j > i and in_box(p, finish))
        return cls(pts, i, j)

    def project(self, p, lo: int, hi: int) -> tuple[float, float]:
        """(distance, progress in route ticks) of the point of the route nearest p, on
        segments lo..hi."""
        best = (math.inf, float(lo))
        for i in range(max(lo, 0), min(hi, len(self.points) - 1)):
            a, b = self.points[i], self.points[i + 1]
            d = [b[k] - a[k] for k in range(3)]
            n = sum(x * x for x in d)
            u = 0.0 if n == 0 else max(0.0, min(1.0, sum((p[k] - a[k]) * d[k] for k in range(3)) / n))
            dist = math.dist(p, [a[k] + u * d[k] for k in range(3)])
            if dist < best[0]:
                best = (dist, i + u)
        return best

    def progress(self, xyz) -> list[float | None]:
        """Each sample's progress, None off the route or after the attempt left it."""
        out: list[float | None] = []
        cur, off = None, 0
        for p in xyz:
            if off >= OFF_ROUTE_END:
                out.append(None)
                continue
            if cur is None:
                d, s = self.project(p, 0, len(self.points))
            else:
                d, s = self.project(p, int(cur) - 2, int(cur) + ROUTE_WINDOW)
            if d > ROUTE_RADIUS:
                out.append(None)
                off += cur is not None
                continue
            off, cur = 0, s if cur is None else max(cur, s)
            out.append(cur)
        return out


def cut(life: players.Life) -> list[list[int]]:
    """A life's pose ticks, cut at teleports and (remote) gaps."""
    ticks = sorted(life.poses)
    parts, cur = [], []
    for t in ticks:
        if cur:
            a, b = life.poses[cur[-1]], life.poses[t]
            dt = t - cur[-1]
            if math.dist((a.x, a.y, a.z), (b.x, b.y, b.z)) > TELEPORT_SPEED * dt or (not life.local and dt > MAX_GAP):
                parts.append(cur)
                cur = []
        cur.append(t)
    if cur:
        parts.append(cur)
    return [p for p in parts if len(p) >= MIN_SAMPLES]


def glide_at(life: players.Life, ticks: list[int], rec_flags=None) -> list[int]:
    out, i, cur = [], 0, 0
    flags = sorted(rec_flags if rec_flags is not None else life.flags)
    for t in ticks:
        while i < len(flags) and flags[i][0] <= t:
            cur = flags[i][1]
            i += 1
        out.append(cur >> fb.FLAG_GLIDE & 1)
    return out


def attempts_of(rec: players.Recording, dimension: str, course: dict, route: Route, gates: list[int]) -> list[dict]:
    start, finish = course["regions"]["start_area"]["working"], course["regions"]["finish"]["working"]
    out = []
    for life in rec.lives:
        if life.dimension != dimension:
            continue
        for ticks in cut(life):
            poses = [life.poses[t] for t in ticks]
            xyz = [(p.x, p.y, p.z) for p in poses]
            prog = route.progress(xyz)
            if not any(s is not None for s in prog):
                continue
            ex = next((i for i in range(1, len(xyz)) if in_box(xyz[i - 1], start) and not in_box(xyz[i], start)
                       and (life.local or ticks[i] - ticks[i - 1] <= GATE_GAP)), None)
            fin = next((i for i in range(ex or 0, len(xyz)) if in_box(xyz[i], finish)), None)
            origin = ticks[ex] if ex is not None else ticks[0]
            times = gate_times(ticks, prog, gates, origin, None if life.local else GATE_GAP)
            reached = max((k for k, v in enumerate(times) if v is not None), default=None)
            v = None
            if ex is not None:
                span = 1 if life.local else V_SPAN
                j = next((j for j in range(ex + 1, len(xyz)) if ticks[j] - ticks[ex] >= span), None)
                if j is not None and ticks[j] - ticks[ex] <= 2 * span:
                    v = [(xyz[j][k] - xyz[ex][k]) / (ticks[j] - ticks[ex]) for k in range(3)]
            out.append(dict(
                id=f"{rec.path.stem}:{rec.name_of(life)}:{ticks[0]}", source=rec.path.name, player=rec.name_of(life),
                local=life.local, first_tick=ticks[0], exit=ex, exit_v=v,
                exit_speed=None if v is None else math.hypot(*v), finish=fin, reached=reached, gates=times,
                t=ticks, xyz=[[round(c, 7) for c in p] for p in xyz],
                yaw=[round(p.yaw, 4) for p in poses], pitch=[round(p.pitch, 4) for p in poses],
                ground=[int(p.on_ground) for p in poses], glide=glide_at(life, ticks),
            ))
    return out


def gate_times(ticks, prog, gates, origin, max_gap=None) -> list[float | None]:
    """The tick (since origin) each gate's progress was first passed, interpolated
    between the samples either side; None for gates from before the first on-route sample,
    never reached, or (`max_gap`) passed in a wider gap between samples."""
    out = []
    samples = [(t, s) for t, s in zip(ticks, prog) if s is not None]
    i = 0
    for g in gates:
        while i < len(samples) and samples[i][1] < g:
            i += 1
        if i == len(samples) or i == 0:
            out.append(None)
            continue
        (t0, s0), (t1, s1) = samples[i - 1], samples[i]
        if max_gap is not None and t1 - t0 > max_gap:
            out.append(None)
            continue
        out.append(t1 - origin if s1 == s0 else t0 + (g - s0) / (s1 - s0) * (t1 - t0) - origin)
    return out


def splits_report(attempts: list[dict], route: Route, gates: list[int]) -> str:
    lines = [f"# Splits against {len(route.points)} route ticks, gates at route ticks {gates}", ""]
    lines += ["## Exits of the start area, fastest first (exit speed in blocks/tick; local exact, remote "
              "over about 4 ticks, so a burst of moves can inflate it: trust the medians)", "",
              "| player | kind | speed | vx | vy | vz | at | attempt |", "|---|---|---|---|---|---|---|---|"]
    ex = sorted((a for a in attempts if a["exit_speed"] is not None), key=lambda a: -a["exit_speed"])
    seen: dict[str, int] = {}
    for a in ex:
        seen[a["player"]] = seen.get(a["player"], 0) + 1
        if seen[a["player"]] <= 8:
            v, p = a["exit_v"], a["xyz"][a["exit"]]
            lines.append(f"| {a['player']} | {'local' if a['local'] else 'remote'} | {a['exit_speed']:.3f} | "
                         f"{v[0]:.3f} | {v[1]:.3f} | {v[2]:.3f} | {p[0]:.1f} {p[1]:.1f} {p[2]:.1f} | {a['id']} |")
    lines += ["", f"{len(ex)} exits: " + ", ".join(f"{k} {v}" for k, v in sorted(seen.items(), key=lambda kv: -kv[1])), ""]
    lines += ["## Best stretch between gates", "",
              "Ticks between consecutive gates, over every attempt that passed both. `best` is who holds it.", "",
              "| gates | route | best | by | local best | by | others within 1 |", "|---|---|---|---|---|---|---|"]
    best_sum = local_sum = 0.0
    for k in range(len(gates) - 1):
        rows = [(a["gates"][k + 1] - a["gates"][k], a) for a in attempts
                if a["gates"][k] is not None and a["gates"][k + 1] is not None and a["gates"][k + 1] > a["gates"][k]]
        if not rows:
            lines.append(f"| {gates[k]}-{gates[k + 1]} | {gates[k + 1] - gates[k]} | no data | | | | |")
            continue
        rows.sort(key=lambda r: r[0])
        b, ba = rows[0]
        loc = [r for r in rows if r[1]["local"]]
        lb, la = loc[0] if loc else (math.inf, None)
        best_sum += b
        local_sum += lb
        near = sorted({a["player"] for d, a in rows if d <= b + 1} - {ba["player"]})
        lines.append(f"| {gates[k]}-{gates[k + 1]} | {gates[k + 1] - gates[k]} | {b:.1f} | {ba['id']} | {lb:.1f} | "
                     f"{la['id'] if la else ''} | {' '.join(near)} |")
    lines += ["", f"Sum of the best stretches: {best_sum:.1f} ticks = {best_sum * 0.05:.2f} s; local-only (exact poses, so "
              f"the fair number): {local_sum:.1f} ticks = {local_sum * 0.05:.2f} s. Each is a lower-bound hint, not a "
              "flyable run: stretches end in different states. Remote stretches can be short by a couple of ticks "
              "of packet timing.", ""]
    lines += ["## Coverage", "", "Attempts reaching each gate, by player (in the player's order of first appearance):", ""]
    who = sorted({a["player"] for a in attempts})
    lines += ["| gate | " + " | ".join(who) + " |", "|---|" + "---|" * len(who)]
    for k, g in enumerate(gates):
        cells = [str(sum(1 for a in attempts if a["player"] == w and a["gates"][k] is not None)) for w in who]
        lines.append(f"| {g} | " + " | ".join(cells) + " |")
    return "\n".join(lines) + "\n"


def write_tapes(rec_path: Path, outdir: Path, dimension: str) -> list[str]:
    """The finished runs in `dimension` of a replay's local player, each from the last rest
    before its drop, as tapes. Returns one line per tape."""
    r = fb.Replay.load(rec_path)
    tag = rec_path.stem.replace("-", "").replace("T", "_")[:13]
    dims = [(e.tick, e.data) for e in r.events if e.kind == "dimension"]
    out = []
    for i, run in enumerate(er.find_runs(r)):
        if [d for t, d in dims if t <= run.finish][-1:] != [dimension]:
            continue
        run.from_last_rest = True
        header, lines, problems, warnings = er.extract(r, run)
        time = run.finish_text.split("time: ")[1].split()[0]
        path = outdir / f"{time}_{tag}_run{i}.tas"
        path.write_text("\n".join(header + lines) + "\n")
        out.append(f"{path.name}: {sum(not x.startswith('@') for x in lines)} ticks, {len(problems)} key problems, "
                   f"{len(warnings)} glide mismatches")
    return out


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("course_dir", type=Path)
    ap.add_argument("replays", nargs="+", type=Path)
    ap.add_argument("-o", "--out", type=Path)
    ap.add_argument("--gate-every", type=int, default=40, help="route ticks between gates")
    ap.add_argument("--tapes", action="store_true", help="also write the local players' finished runs as tapes")
    args = ap.parse_args(argv)

    course = json.loads((args.course_dir / "course.json").read_text())
    root = Path(__file__).resolve().parents[1]
    route = Route.from_tape(root / course["start"]["source"]["path"], course)
    gates = list(range(route.exit, route.finish, args.gate_every)) + [route.finish]
    out = args.out or args.course_dir / "runs"
    out.mkdir(parents=True, exist_ok=True)

    attempts = []
    for p in args.replays:
        rec = players.load(p)
        got = attempts_of(rec, course["dimension"], course, route, gates)
        print(f"{p.name}: {len(got)} attempts on the route", file=sys.stderr)
        attempts += got
        if args.tapes:
            (out / "seeds").mkdir(exist_ok=True)
            for line in write_tapes(p, out / "seeds", course["dimension"]):
                print(" ", line, file=sys.stderr)
    with gzip.open(out / "segments.jsonl.gz", "wt") as f:
        for a in attempts:
            f.write(json.dumps(a, separators=(",", ":")) + "\n")
    (out / "splits.md").write_text(splits_report(attempts, route, gates))
    print(f"wrote {out}/segments.jsonl.gz ({len(attempts)} attempts), {out}/splits.md", file=sys.stderr)


if __name__ == "__main__":
    main()
