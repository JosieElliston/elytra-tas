"""EMC boosters, measured from logger logs and Flashback replays.

usage: python3 tools/boosters.py scan [--logs GLOB] [--replays GLOB]
       python3 tools/boosters.py visits LOG.tsv.gz|REPLAY.zip [--speed S] [--box X0 Y0 Z0 X1 Y1 Z1]
       python3 tools/boosters.py fixture COURSE -o OUT.json

`scan` classifies every `set_entity_motion` of the local player in every log and
replay: a booster velocity is one that `payload` reproduces bit for bit from the
rotation of a move sent up to MAX_DELAY ticks before (the speed estimated from the
ratio to the look); the rest are wall-hit echoes and the like. It prints, per map,
the speeds and delays. `visits` prints each pass through a box: per tick inside,
whether a boost carried that tick's move's rotation (T; S when the booster sound came
back for it, logs only) or not (.). `fixture` writes the moves near a course's
boosters and the boosts recorded over them, for `solver/crates/etas/tests/booster.rs`.
fit_course.py fits a course's boosters from `booster_points` and `visit_stats`.

What the data says (alpine logs of 2026-10-20 01:26-02:06 and the 00:09 replay;
docs/emc-course-rules.md has the summary):
- payload: `payload(speed, yaw, pitch)`: Bukkit's `Location.getDirection` (double trig
  of the float rotation, yaw wrapped as the server holds it) times the speed, clamped
  to +-3.9 and truncated to 1/8000 per axis (the pre-1.21.9 velocity packet), then
  LpVec3-quantized (ViaVersion re-encoding it for 26.2). The 31 alpine motions of
  the 01:58 log it doesn't reproduce all have an axis exactly 0 (wall-hit echoes);
  without the 1/8000 step fewer than a third of the 206 it does would match. The rotation is that of the move whose position was inside.
- delay: the boost acts on the physics of the tick `delay` after that move's tick: the
  booster sound comes back with the move's tick-end reply, the velocity one server
  tick later, so 3 or 4 by session (rarely 5-7: lag), and it can change within a pass.
- trigger: not a function of the moves. Passes through the same spot at the same
  speed differ: the first boost comes 0-3 ticks after entering the box, the next two
  ticks boost or not, every later tick inside does. Move timing was regular in the
  passes checked (one move per server tick), so it isn't two moves sharing a tick.
  Standing in a booster (on the ground) boosted every 10-25 ticks instead.
"""

from __future__ import annotations

import argparse
import glob
import json
import math
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

import flashback as fb
import netlog

PROFILE = Path.home() / "Library/Application Support/ModrinthApp/profiles/main"
REPLAYS = PROFILE / "flashback/replays"
LOGS = PROFILE / "elytra-tas/logs"

MAX_DELAY = 8  # ticks back from an arrival searched for the rotation
CLAMP = 3.9  # the old velocity packet's clamp


# --- payload ------------------------------------------------------------------------


def wrap_degrees(y: float) -> float:
    """Mth.wrapDegrees(float), as the server applies it to a received yaw."""
    f = np.fmod(np.float32(y), np.float32(360.0))  # Java's float %
    if f >= np.float32(180.0):
        f -= np.float32(360.0)
    if f < np.float32(-180.0):
        f += np.float32(360.0)
    return float(f)


def look(yaw: float, pitch: float) -> tuple[float, float, float]:
    """Bukkit's Location.getDirection: double trig of the float rotation."""
    xz = math.cos(math.radians(pitch))
    return (-xz * math.sin(math.radians(yaw)), -math.sin(math.radians(pitch)), xz * math.cos(math.radians(yaw)))


def short8000(v: tuple[float, float, float]) -> tuple[float, float, float]:
    """The pre-1.21.9 ClientboundSetEntityMotionPacket: `(int)(clamp(v, -3.9, 3.9) * 8000)`
    per axis, read back as `/ 8000.0`."""
    return tuple(int(max(-CLAMP, min(CLAMP, c)) * 8000.0) / 8000.0 for c in v)


def lpvec3(v: tuple[float, float, float]) -> tuple[float, float, float]:
    """net.minecraft.network.LpVec3 write then read: what the 26.2 client decodes."""
    m = max(abs(c) for c in v)
    if m < 3.051944088384301e-5:
        return (0.0, 0.0, 0.0)
    s = math.ceil(m)
    out = []
    for c in v:
        x = (c / s * 0.5 + 0.5) * 32766.0
        q = math.floor(x) + (x - math.floor(x) >= 0.5)  # Java's Math.round, x >= 0
        q &= 32767
        out.append((min(q, 32766.0) * 2.0 / 32766.0 - 1.0) * s)
    return tuple(out)


def payload(speed: float, yaw: float, pitch: float) -> tuple[float, float, float]:
    """The velocity the client receives from a booster of `speed` for this rotation
    (as sent; the server wraps the yaw)."""
    d = look(wrap_degrees(yaw), pitch)
    return lpvec3(short8000(tuple(speed * c for c in d)))


def speeds_of(v, rot) -> list[float]:
    """Candidate booster speeds for a received velocity and a rotation: the ratio of
    velocity to look on the axes the clamp can't have touched, to 1 and 2 decimals."""
    d = look(wrap_degrees(rot[0]), rot[1])
    est = [c / l for c, l in zip(v, d) if abs(c) < CLAMP - 0.01 and abs(l) > 0.05]
    if not est:
        return []
    e = sorted(est)[len(est) // 2]
    return sorted({round(e, 1), round(e, 2)} - {0.0})


# --- sources ------------------------------------------------------------------------


@dataclass
class Move:
    """The last move packet a client tick sent, with the state the server then holds."""
    tick: int  # client tick (tape numbering for replays, logger ticks for logs)
    t: float  # send time, ns (logs) or tick (replays)
    pos: tuple[float, float, float]
    rot: tuple[float, float]  # yaw, pitch as the server holds them (sent, unwrapped)
    on_ground: bool


@dataclass
class Motion:
    arrival: int  # the client tick whose physics it acts on
    t: float  # decode time, ns (logs) or tick (replays)
    v: tuple[float, float, float]


@dataclass
class Boost:
    motion: Motion
    speed: float
    move: Move  # the move whose rotation it carries
    delay: int  # motion.arrival - move.tick
    unique: bool  # no other move in the search window gives the same payload


@dataclass
class Session:
    name: str
    kind: str  # "log" or "replay"
    dims: list  # (tick, dimension) changes
    moves: dict[int, Move]
    motions: list[Motion]
    sounds: list[float] = field(default_factory=list)  # decode times, logs only

    def dim(self, tick: int) -> str | None:
        cur = None
        for t, d in self.dims:
            if t <= tick:
                cur = d
        return cur


def log_session(path: Path) -> Session:
    log = netlog.load(path)
    moves, pos, rot = {}, None, None
    for o in log.outbound:
        m = o.move()
        if m is None:
            continue
        if m["yrot"] is not None:
            rot = (m["yrot"], m["xrot"])
        if m["x"] is not None:
            pos = (m["x"], m["y"], m["z"])
        if pos is not None and rot is not None:
            moves[o.tick] = Move(o.tick, o.t_send, pos, rot, m["on_ground"])
    dims = sorted((i.handle_tick, i.details[0]) for i in log.inbound
                  if i.name in ("login", "respawn") and i.details and i.handle_tick is not None)
    motions = [Motion(i.handle_tick + 1, i.t_decode, tuple(float(x) for x in i.details[1:4]))
               for i in log.inbound if i.name == "set_entity_motion" and i.details[:1] == ("self",)
               and i.handle_tick is not None]
    sounds = [i.t_decode for i in log.inbound if i.name == "sound"]
    return Session(path.name, "log", dims, moves, motions, sounds)


def replay_session(path: Path) -> Session:
    """Moves are the recorded tick-end poses (what that tick's move packet carried);
    a motion recorded in tick t was handled before tick t's physics."""
    r = fb.Replay.load(path)
    dims = []
    for e in r.events:
        if e.kind in ("login", "respawn"):
            dims.append((e.tick, f"#{len(dims)}"))  # flashback.py keeps no dimension names
    moves, cur = {}, None
    for t in range(r.total_ticks):
        cur = r.poses.get(t, cur)
        if cur is not None:
            moves[t] = Move(t, t, (cur.x, cur.y, cur.z), (cur.yaw, cur.pitch), cur.on_ground)
    motions = [Motion(e.tick, e.tick, tuple(e.data)) for e in r.events if e.kind == "motion"]
    return Session(path.name, "replay", dims, moves, motions)


def classify(s: Session) -> tuple[list[Boost], list[Motion]]:
    """Booster velocities (reproduced by `payload` from a move's rotation within
    MAX_DELAY ticks before the arrival) and the rest. Where several moves give the
    payload (a rotation held for ticks), the one at the session's usual delay wins:
    the commonest delay among the unambiguous matches."""
    found_all = []
    for m in s.motions:
        found = []
        for d in range(1, MAX_DELAY + 1):
            mv = s.moves.get(m.arrival - d)
            if mv is None:
                continue
            for sp in speeds_of(m.v, mv.rot):
                if payload(sp, *mv.rot) == m.v:
                    found.append((d, sp, mv))
        found_all.append((m, found))
    usual = Counter(f[0][0] for m, f in found_all if len({x[2].rot for x in f}) == 1 and f)
    d0 = usual.most_common(1)[0][0] if usual else 4
    boosts, other = [], []
    for m, found in found_all:
        if not found:
            other.append(m)
            continue
        d, sp, mv = min(found, key=lambda f: (abs(f[0] - d0), f[0]))
        boosts.append(Boost(m, sp, mv, d, len({f[2].rot for f in found}) == 1))
    return boosts, other


# --- visits ---------------------------------------------------------------------------


def in_box(p, box) -> bool:
    return all(box[0][k] <= p[k] <= box[1][k] for k in range(3))


def visits(s: Session, boosts: list[Boost], box, speed: float):
    """Runs of consecutive ticks whose move is inside `box`, each tick marked: T a boost
    of `speed` carries that move's rotation, S the same with a booster sound back
    within 80-150 ms of the move (logs), . none."""
    trig = {b.move.tick for b in boosts if b.speed == speed}
    runs, cur = [], []
    for t in sorted(s.moves):
        if in_box(s.moves[t].pos, box):
            if cur and t != cur[-1] + 1:
                runs.append(cur)
                cur = []
            cur.append(t)
        elif cur:
            runs.append(cur)
            cur = []
    if cur:
        runs.append(cur)
    out = []
    for run in runs:
        marks = []
        for t in run:
            snd = s.kind == "log" and any(80e6 < x - s.moves[t].t < 150e6 for x in s.sounds)
            marks.append(("S" if snd else "T") if t in trig else ".")
        out.append((run, "".join(marks)))
    return out


SETTLE = 3  # boosts are sure from this many inside ticks after a visit's first one


def booster_points(s: Session, boosts: list[Boost], speed: float, search) -> tuple[list, list]:
    """Region evidence for one booster, independent of any fitted box: inside, the
    position of every move a boost of `speed` carries the rotation of (within
    `search`); outside, the move after a visit's last boost when that is SETTLE or
    more ticks after the visit's first (boosts are sure there, so no boost means
    outside). A visit is a run of boosted moves with gaps of at most SETTLE - 1."""
    trig = sorted({b.move.tick for b in boosts if b.speed == speed and in_box(b.move.pos, search)})
    inside = [(s.moves[t].pos, f"{s.name}@{t}") for t in trig]
    outside = []
    groups = []
    for t in trig:
        if groups and t - groups[-1][-1] <= SETTLE:
            groups[-1].append(t)
        else:
            groups.append([t])
    for g in groups:
        nxt = g[-1] + 1
        if any(s.moves[t].on_ground for t in range(g[0], nxt + 1) if t in s.moves):
            continue  # the rules don't hold on the ground
        if nxt - g[0] >= SETTLE and nxt in s.moves:
            outside.append((s.moves[nxt].pos, f"{s.name}@{nxt} (after {g[0]}..{g[-1]})"))
    return inside, outside


def visit_stats(s: Session, boosts: list[Boost], working, inner, speed: float) -> dict:
    """Per visit of `working` (consecutive inside ticks), the rules a course booster
    states (fit_course.py's docstring): the first boost's index after the visit's first
    tick inside `inner` (entry lag), the boosts on the SETTLE - 1 ticks after the first
    (maybe ticks: boosted or not), and each tick inside `inner` at or after first +
    SETTLE without a boost (a sure miss). Visits with a move on the ground are only
    counted (`grounded`): the rules don't hold there."""
    lags, misses, never, maybe, grounded = Counter(), [], 0, Counter(), Counter()
    for run, marks in visits(s, boosts, working, speed):
        if any(s.moves[t].on_ground for t in run):
            grounded["boosts" if marks.strip(".") else "none"] += 1
            continue
        f = next((i for i, c in enumerate(marks) if c != "."), None)
        if f is None:
            never += 1
            continue
        e = next((i for i, t in enumerate(run) if in_box(s.moves[t].pos, inner)), f)
        lags[max(f - e, 0)] += 1
        for i in range(f + 1, min(f + SETTLE, len(marks))):
            maybe[marks[i] != "."] += 1
        misses += [f"{s.name}@{run[i]}" for i in range(f + SETTLE, len(marks))
                   if marks[i] == "." and in_box(s.moves[run[i]].pos, inner)]
    return dict(lags=lags, misses=misses, never=never, maybe=maybe, grounded=grounded)


# --- commands -------------------------------------------------------------------------


def cmd_scan(args):
    paths = [Path(p) for p in sorted(glob.glob(str(LOGS / args.logs)))]
    paths += [Path(p) for p in sorted(glob.glob(str(REPLAYS / args.replays)))]
    for p in paths:
        if p.stat().st_size > args.max_mb * 1e6:
            print(f"{p.name}: skipped ({p.stat().st_size / 1e6:.0f} MB)")
            continue
        try:
            s = log_session(p) if p.suffix == ".gz" else replay_session(p)
        except Exception as e:  # noqa: BLE001 - report and go on
            print(f"{p.name}: unreadable ({e})")
            continue
        boosts, other = classify(s)
        if not s.motions:
            continue
        per = defaultdict(lambda: [Counter(), Counter(), 0])
        for b in boosts:
            k = s.dim(b.motion.arrival)
            per[k][0][b.speed] += 1
            per[k][1][b.delay] += 1
        for m in other:
            per[s.dim(m.arrival)][2] += 1
        for k, (spd, dl, n_other) in per.items():
            print(f"{p.name} {k}: boosts {sum(spd.values())} speeds {dict(spd)} delays {dict(sorted(dl.items()))}; "
                  f"other motions {n_other}")


def parse_box(v):
    return ((v[0], v[1], v[2]), (v[3], v[4], v[5]))


def cmd_visits(args):
    s = log_session(Path(args.log)) if args.log.endswith(".gz") else replay_session(Path(args.log))
    boosts, other = classify(s)
    print(f"{len(boosts)} boosts, {len(other)} other motions; delays {dict(Counter(b.delay for b in boosts))}")
    if args.box:
        for run, marks in visits(s, boosts, parse_box(args.box), args.speed):
            print(run[0], marks)


def course_sessions(cfg: dict) -> list[tuple[Session, list[Boost]]]:
    """The booster sessions of a fit_course.py COURSES entry (`booster_logs`, then
    `booster_replays`), classified; log sessions keep only the course's dimension."""
    out = []
    for name in cfg.get("booster_logs", []):
        s = log_session(LOGS / name)
        keep = {t for t in s.moves if s.dim(t) == cfg["dimension"]}
        s.moves = {t: m for t, m in s.moves.items() if t in keep}
        s.motions = [m for m in s.motions if s.dim(m.arrival) == cfg["dimension"]]
        out.append((s, classify(s)[0]))
    for name in cfg.get("booster_replays", []):
        s = replay_session(REPLAYS / name)
        out.append((s, classify(s)[0]))
    return out


MARGIN = 3.0  # blocks around a booster's search box, for fixture windows
PAD = 3  # ticks before and after


def cmd_fixture(args):
    """Every pass near each booster of a course: the moves (tick, x, y, z, yaw and pitch
    as hex float bits, as the server holds them, on_ground 0/1) and the booster velocities recorded over them (arrival tick,
    v, and the tick of the move whose rotation it carries). Windows are runs of ticks
    whose move is within MARGIN of the booster's search box, padded by PAD ticks.
    Independent of any fitted region, so a refit doesn't need a new fixture."""
    import fit_course
    cfg = fit_course.COURSES[args.course]
    fbits = lambda x: f"{np.float32(x).view(np.uint32).item():08x}"  # noqa: E731
    out = {"course": args.course, "tool": "boosters.py fixture", "sources": [], "windows": []}
    for s, boosts in course_sessions(cfg):
        out["sources"].append(s.name)
        for spec in cfg["boosters"]:
            lo, hi = spec["search"]
            near = ([c - MARGIN for c in lo], [c + MARGIN for c in hi])
            ticks = sorted(t for t, m in s.moves.items() if in_box(m.pos, near))
            groups = []
            for t in ticks:
                if groups and t - groups[-1][-1] <= PAD:
                    groups[-1].append(t)
                else:
                    groups.append([t])
            for g in groups:
                t0, t1 = g[0] - PAD, g[-1] + PAD
                moves = [[t, *s.moves[t].pos, *(fbits(r) for r in s.moves[t].rot), int(s.moves[t].on_ground)]
                         for t in range(t0, t1 + 1) if t in s.moves]
                arr = [[b.motion.arrival, *b.motion.v, b.move.tick] for b in boosts
                       if b.speed == spec["speed"] and t0 <= b.move.tick <= t1]
                if arr or any(in_box(m[1:4], (lo, hi)) for m in moves):
                    out["windows"].append(dict(source=s.name, booster=spec["name"], moves=moves, arrivals=arr))
    Path(args.out).write_text(json.dumps(out, separators=(",", ":")) + "\n")
    n = sum(len(w["arrivals"]) for w in out["windows"])
    print(f"wrote {args.out}: {len(out['windows'])} windows, {n} booster arrivals")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("scan")
    a.add_argument("--logs", default="*.tsv.gz")
    a.add_argument("--replays", default="*.zip")
    a.add_argument("--max-mb", type=float, default=60)
    a.set_defaults(fn=cmd_scan)
    a = sub.add_parser("visits")
    a.add_argument("log")
    a.add_argument("--box", type=float, nargs=6)
    a.add_argument("--speed", type=float, default=1.4)
    a.set_defaults(fn=cmd_visits)
    a = sub.add_parser("fixture")
    a.add_argument("course")
    a.add_argument("-o", "--out", required=True)
    a.set_defaults(fn=cmd_fixture)
    args = ap.parse_args(argv)
    args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
