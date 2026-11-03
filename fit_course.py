"""Fit a course file (EMC's rules for one map) from Flashback replays and logger logs.

usage: python3 fit_course.py COURSE -o OUT.json [--world WORLD.gz] [--tape TAPE.tas]

COURSE names an entry of COURSES below (its sources: replays, logs, tape, world). The
fit's evidence and brackets go to stdout. `solver/crates/etas` reads the file.

Course file format, version 1. This docstring is the spec; `etas::course` reads it.
One JSON object:

    format          "etas-course"
    version         1; a reader rejects other versions
    name            course name
    dimension       dimension key the sources were matched on
    world           {"path": world file, relative to the course file,
                     "content_sha256": the world header's content_sha256,
                     "extract": the extract_world.py command that wrote it}
                    A reader rejects a world whose hash differs.
    fitted_from     {"replays": [{"name", "sha256"}], "logs": [{"name", "sha256"}],
                     "excluded": [{"name", "why"}], "tool": "fit_course.py",
                     "commit", "dirty", "fitted_at"}
    observation     how a client tick's pose becomes server-observed state (below)
    start           the recorded prefix the solver starts from (below)
    regions         {name: region}; rules refer to them by name (below)
    rules           {"timer_start": rule, "finish": rule, "kill": [rule, ...]}
    ground_allowed  [region names]: where the ground may be touched while the timer
                    runs. Separate from the timer-start region. why? secrets you can
                    stand on and odd start areas exist on other maps.
    boosters        [] (format room; not modeled yet)

Observed state. The server judges the position, onGround and glide flag of a client's
move packets, so a rule is a predicate over (x, y, z, on_ground, fall_flying) of a
tick's end pose, which is what that tick's move packet carries. `observation` records:
    tick_chat_lag   [lo, hi]: a server message batched at a server tick end (timer,
                    finish) reaches the client this many ticks after the end of the tick
                    whose pose it judged, in the .tas tape's tick numbering (a chat on
                    tape tick C judged the pose of tick C - L). From logger round trips;
                    see `lag` in the fit output for the per-session numbers.
    note            free text

Start:
    source          the tape (path relative to the repo root) and its sha256
    pose            [x, y, z, yaw, pitch] as hex float bits (doubles, then floats), the
                    pose after the reset teleport, at rest
    inputs          [[yaw_bits, pitch_bits, keys], ...] for tape ticks 1..glide_tick,
                    keys as in .tas; the deploy is the last one
    glide_tick, timer_tick, finish_tick, finish_text: the tape's header values

Region: an axis-aligned box over the observed position (feet), min <= p <= max on each
axis (a reader takes a null bound as unbounded; the fit writes none). Every region
carries its bracket:
    working         {"min": [x, y, z], "max": [x, y, z]}: the box rules use
    inner           the hull of points the data puts inside (may be degenerate)
    outer           the largest box around inner holding no point the data puts
                    outside, grown on all faces at once and each face stopped at the
                    first such point, or 64 blocks out; not unique (another growth
                    order differs)
    faces           {"x_min": "fitted" | "assumed", ...}: fitted is the midpoint of
                    inner and outer; assumed is inner moved 1 block out, where outer
                    stopped at 64 blocks (the data says nothing about that face)
    status          "fitted" if every face is, else "provisional"
    evidence        counts and source points

Rule:
    event           "first": fires on the first observed state where `when` holds;
                    "any": on every one
    when            predicate: {"in": region} | {"not": p} | {"and": [p...]} |
                    {"or": [p...]} | {"field": "on_ground" | "fall_flying"}
    after, until    rule names bounding the ticks it is checked on ("start" is tick 1)
    status          "fitted" | "provisional", and why in `note`
    evidence        what the fit saw

Killing blocks are the world's (`world::Effect::Climbable` ends the glide, so it kills
here, as unknown space does); kill rules here are the course's own.

How events become evidence (`judged`): a timer or finish chat at tape tick C judged the
pose of C - L for some L in the lag band; poses before the band were judged by earlier
server ticks, so they were on the other side of the rule; the band's latest pose is
taken to be on the far side (the rule's region is entered, or left, and stays so for a
tick). The pose just before the band is left out, since a move that shares a server
tick gap with the next is never judged. With a logger log, the band is the moves sent
D_lo..D_hi before the chat was decoded (D_lo: the fastest round trip a plugin answered
from a packet handler, less 10 ms; D_hi: the slowest tick-end reply plus a tick, both
from `use_item`; the tick covers server lag, which delays tick-end replies).
"""

from __future__ import annotations

import argparse
import bisect
import datetime
import gzip
import hashlib
import json
import math
import struct
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

import flashback as fb
import netlog
import world

ROOT = Path(__file__).resolve().parent
PROFILE = Path.home() / "Library/Application Support/ModrinthApp/profiles/main"
REPLAYS = PROFILE / "flashback/replays"
LOGS = PROFILE / "elytra-tas/logs"

COURSES = {
    "burrowed_grove": dict(
        dimension="minecraft:eburrowedgrove",
        tape="tas/burrowed_grove.tas",
        world="solver/courses/burrowed_grove/burrowed_grove.world.gz",
        world_extract=("python3 extract_world.py <replays>/2026-10-19T23_03_50.zip --tick 24788 "
                       "--bounds -90 -64 -66 45 190 100 -o solver/courses/burrowed_grove/burrowed_grove.world.gz"),
        # replays without a log of the same session
        replays=["2026-10-19T23_03_50.zip", "2026-10-19T23_25_42_ebounce.zip",
                 "2026-10-20T00_00_14_burrowed_grove_three.zip", "2026-10-20T00_18_41.zip",
                 "2026-10-20T00_21_02.zip"],
        # logs; each replaces the replay of the same session (named for provenance)
        logs={"2026-10-20_01-24-23.tsv.gz": "2026-10-20T01_25_47.zip",
              "2026-10-20_01-41-33.tsv.gz": "2026-10-20T01_44_58_burrowed_grove.zip",
              "2026-10-20_18-05-35.tsv.gz": "2026-10-20T18_24_43.zip",
              "2026-10-20_18-26-16.tsv.gz": "2026-10-20T19_14_21.zip"},
        excluded={"2026-10-20_02-50-17.tsv.gz / 2026-10-20T08_50_11_overnight.zip":
                  "unfocused client: ticks and moves go out in pairs, so the one-pose-per-"
                  "server-tick model doesn't hold (README, EMC course rules); its ~600 runs "
                  "replay the tape, so their poses add nothing to the geometry"},
    ),
}

START_RADIUS = 2.0  # a pose within this of the start pose, after a jump, is a reset
REPLAY_LAG = (2, 5)  # ticks; for replays without a log (logged sessions, one tick wider)
DEFAULT_D = (66e6, 160e6)  # ns, when a log has no use_item round trips
GROW_STEP = 0.01  # blocks
GROW_MAX = 64.0  # blocks; a face that grows this far stops there, marked "limit"


def sha256_file(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def fbits(v: float) -> str:
    return struct.pack(">f", v).hex()


def dbits(v: float) -> str:
    return struct.pack(">d", v).hex()


# --- observations ------------------------------------------------------------------


@dataclass
class Obs:
    """A pose the server may have judged: a move packet, or a replay's tick-end pose."""
    key: float  # time order: ns for logs, tick for replays
    tick: int
    pos: tuple[float, float, float]
    on_ground: bool


@dataclass
class Chat:
    key: float  # decode time (logs) or tick (replays)
    tick: int
    text: str
    used_item: bool | None = None  # a use_item went out shortly before (logs only)


@dataclass
class Session:
    name: str
    kind: str  # "log" or "replay"
    obs: list[Obs]
    chats: list[Chat]
    resets: list[float]  # keys of reset teleports
    d_lo: float = 0.0
    d_hi: float = 0.0
    lag_note: str = ""

    def band(self, chat: Chat) -> tuple[list[int], int | None]:
        """Indices into obs of the lag band for `chat` (oldest first), and the index just
        before the band (left out)."""
        keys = [o.key for o in self.obs]
        if self.kind == "log":
            lo = bisect.bisect_left(keys, chat.key - self.d_hi)
            hi = bisect.bisect_right(keys, chat.key - self.d_lo)
            # the latest move older than d_hi is judged if nothing in the band was
            idx = list(range(max(lo - 1, 0), hi))
        else:
            idx = [i for i, o in enumerate(self.obs)
                   if chat.tick - REPLAY_LAG[1] <= o.tick <= chat.tick - REPLAY_LAG[0]]
        before = idx[0] - 1 if idx and idx[0] > 0 else None
        return idx, before


def replay_session(name: str, dim: str) -> Session:
    path = REPLAYS / name
    r = fb.Replay.load(path)
    scan = world.scan_replay(path)
    dims = [(t, d[1]) for t, k, d in scan.events if k == "dimension"]

    def in_dim(t):
        cur = None
        for tt, d in dims:
            if tt <= t:
                cur = d
        return cur == dim

    obs, cur = [], None
    for t in range(r.total_ticks):
        cur = r.poses.get(t, cur)
        if cur is not None and in_dim(t):
            obs.append(Obs(t, t, (cur.x, cur.y, cur.z), cur.on_ground))
    chats = [Chat(e.tick, e.tick, e.data) for e in r.events if e.kind == "chat" and in_dim(e.tick)]
    return Session(name, "replay", obs, chats, resets_of(obs), lag_note=f"replay band {REPLAY_LAG}")


def resets_of(obs: list[Obs]) -> list[float]:
    """Keys where the pose jumps onto the start (the reset teleport)."""
    if not obs:
        return []
    out = []
    for a, b in zip(obs, obs[1:]):
        if math.dist(a.pos, b.pos) > 2.0 and math.dist(b.pos, START) < START_RADIUS:
            out.append(b.key)
    return out


def log_session(name: str, dim: str) -> Session:
    log = netlog.load(LOGS / name)
    # the dimension timeline, from login/respawn handled packets
    dims = []
    for i in log.inbound:
        if i.name in ("login", "respawn") and i.t_handle and i.details:
            dims.append((i.t_handle, i.details[0]))
    dims.sort()

    def in_dim(t):
        cur = None
        for tt, d in dims:
            if tt <= t:
                cur = d
        return cur == dim

    obs, last = [], None
    for o in log.outbound:
        m = o.move()
        if m is None:
            continue
        if m["x"] is not None:
            last = (m["x"], m["y"], m["z"])
        if last is not None and in_dim(o.t_send):
            obs.append(Obs(o.t_send, o.tick, last, m["on_ground"]))
    uses = sorted(o.t_send for o in log.outbound if o.name == "use_item")
    chats = []
    for i in log.inbound:
        if i.name == "system_chat" and i.details and i.t_handle and in_dim(i.t_decode):
            k = bisect.bisect_left(uses, i.t_decode)
            used = k > 0 and i.t_decode - uses[k - 1] < 1000e6
            chats.append(Chat(i.t_decode, i.handle_tick, i.details[-1], used))
    # round trips of use_item: the plugin's packet-handler reply (player_position) and
    # the vanilla tick-end reply (block_changed_ack)
    fast, slow = [], []
    for u in uses:
        if not in_dim(u):
            continue
        for kind, acc in (("player_position", fast), ("block_changed_ack", slow)):
            j = next((i for i in log.inbound if i.name == kind and i.t_decode > u), None)
            # under 40 ms is a teleport the server sent anyway, not a reply
            if j is not None and 40e6 < j.t_decode - u < 250e6:
                acc.append(j.t_decode - u)
    # the fastest reply seen bounds the round trip from above only, so the band reaches
    # down to the fastest of any EMC session (66 ms, 2026-10-20 01:41) less a margin
    d_lo = min(min(fast, default=DEFAULT_D[0]), 66e6) - 10e6
    d_hi = max(slow) + 60e6 if slow else DEFAULT_D[1]
    note = (f"use_item replies: handler {len(fast)} (min {d_lo / 1e6:.0f} ms), tick end {len(slow)} "
            f"(max {max(slow) / 1e6 if slow else float('nan'):.0f} ms); band {d_lo / 1e6:.0f}..{d_hi / 1e6:.0f} ms")
    resets = [i.t_handle for i in log.inbound if i.name == "player_position" and i.details and in_dim(i.t_handle)
              and math.dist(tuple(float(v) for v in i.details[1:4]), START) < START_RADIUS]
    return Session(name, "log", obs, chats, sorted(resets), d_lo, d_hi, note)


# --- evidence ----------------------------------------------------------------------


@dataclass
class Evidence:
    start_in: list = field(default_factory=list)  # (pos, source)
    start_out: list = field(default_factory=list)
    finish_in: list = field(default_factory=list)
    finish_out: list = field(default_factory=list)
    touches: list = field(default_factory=list)  # dicts
    events: dict = field(default_factory=lambda: {"timer": 0, "finish": 0, "attempts": 0})
    lag: list = field(default_factory=list)
    runs: list = field(default_factory=list)  # completed runs: (session, attempt start index, timer chat, finish chat)


def is_end(text: str) -> bool:
    return text.startswith("Your attempt lasted") or text.startswith("Course complete!")


def collect(s: Session, ev: Evidence):
    ev.lag.append({"session": s.name, "kind": s.kind, "note": s.lag_note})
    obs = s.obs
    keys = [o.key for o in obs]
    chats = [c for c in s.chats if c.text == "Timer started!" or is_end(c.text)]
    bounds = sorted(s.resets) + [math.inf]
    src = s.name
    # attempts: from each reset to the next
    for a, b in zip([-math.inf] + bounds[:-1], bounds):
        if a == -math.inf:
            continue
        ia, ib = bisect.bisect_left(keys, a), bisect.bisect_left(keys, b)
        cs = [c for c in chats if a <= c.key < b]
        timer = next((c for c in cs if c.text == "Timer started!"), None)
        end = next((c for c in cs if is_end(c.text) and (timer is None or c.key > timer.key)), None)
        ev.events["attempts"] += 1
        if timer is None:
            # never left the start area (by the rule), up to the next reset or attempt end
            stop = ib
            for c in cs:
                if is_end(c.text):
                    band, before = s.band(c)
                    stop = min(stop, before if before is not None else stop)
            ev.start_in += [(obs[i].pos, f"{src}@{obs[i].tick}") for i in range(ia, stop)]
            touches(s, ia, stop, None, None, ev, timer_running=False)
            continue
        ev.events["timer"] += 1
        band, before = s.band(timer)
        band = [i for i in band if i >= ia]
        if not band:
            continue
        ev.start_in += [(obs[i].pos, f"{src}@{obs[i].tick}") for i in range(ia, before or ia)]
        ev.start_out.append((obs[band[-1]].pos, f"{src}@{obs[band[-1]].tick} (timer chat @{timer.tick})"))
        touches(s, ia, band[0], None, None, ev, timer_running=False)
        t0 = band[-1]
        if end is None:
            continue
        eband, ebefore = s.band(end)
        if end.text.startswith("Course complete!"):
            ev.events["finish"] += 1
            ev.runs.append((s, ia, timer, end, eband))
            ev.finish_out += [(obs[i].pos, f"{src}@{obs[i].tick}") for i in range(t0, ebefore or t0)]
            touches(s, t0, eband[0], end, None, ev, timer_running=True)
        else:
            ev.finish_out += [(obs[i].pos, f"{src}@{obs[i].tick}") for i in range(t0, ebefore or t0)]
            touches(s, t0, ib, end, end, ev, timer_running=True)


def timed(text: str) -> list[int]:
    """Server ticks a 'Your time: X seconds' can stand for: EMC's timer is wall-clock
    time between two server ticks, shown to 0.01 s and observed to fall within 0.02 s of
    a whole number of ticks (README, EMC course rules)."""
    x = float(text.split("Your time:")[1].split()[0])
    return [n for n in range(round(x / 0.05) - 1, round(x / 0.05) + 2) if abs(n * 0.05 - x) <= 0.025]


def in_box(p, box) -> bool:
    lo, hi = box["min"], box["max"]
    return all((lo[k] is None or lo[k] <= p[k]) and (hi[k] is None or p[k] <= hi[k]) for k in range(3))


def timed_finishes(ev: Evidence, start_area: dict):
    """Finish evidence from completed runs, anchored on the timer: the timer started on
    the first pose outside the start area, and the finish was judged that many server
    ticks (`timed`) later, one pose per tick in a steady session. Runs where the inner
    and outer start boxes disagree on the timer pose, or the anchored pose falls outside
    the finish chat's lag band, fall back to the band (its latest pose inside)."""
    for s, ia, timer, end, eband in ev.runs:
        obs, src = s.obs, s.name
        band, _ = s.band(timer)
        exits = []
        for which in ("inner", "outer"):
            i = next((i for i in range(ia, band[-1] + 1) if not in_box(obs[i].pos, start_area[which])), None)
            exits.append(i)
        ns = timed(end.text)
        anchored = exits[0] is not None and exits[0] == exits[1] and all(exits[0] + n in eband for n in ns)
        if anchored:
            first, last = exits[0] + min(ns), exits[0] + max(ns)
            ev.finish_in.append((obs[last].pos, f"{src}@{obs[last].tick} (timer pose @{obs[exits[0]].tick} + {max(ns)})"))
            ev.finish_out.append((obs[first - 1].pos, f"{src}@{obs[first - 1].tick} (timer-anchored)"))
            ev.events.setdefault("finish_anchored", 0)
            ev.events["finish_anchored"] += 1
        else:
            ev.finish_in.append((obs[eband[-1]].pos, f"{src}@{obs[eband[-1]].tick} (finish chat @{end.tick})"))


def touches(s: Session, i0: int, i1: int, end: Chat | None, reset: Chat | None, ev: Evidence, timer_running: bool):
    """Ground touches (runs of on_ground observations) in obs[i0:i1] and their outcome."""
    i = i0
    while i < i1:
        if not s.obs[i].on_ground:
            i += 1
            continue
        j = i
        while j < i1 and s.obs[j].on_ground:
            j += 1
        o = s.obs[i]
        outcome = "survived"
        if reset is not None and reset.key >= o.key:
            dt = reset.key - o.key
            near = dt < (600e6 if s.kind == "log" else 12)
            if near:
                outcome = ("reset by item (unknown)" if reset.used_item else "killed") if s.kind == "log" else "reset soon after (unknown)"
        if outcome != "survived" and any(t.get("reset") is reset for t in ev.touches):
            outcome = "same reset as the touch before"
        ev.touches.append(dict(reset=reset if outcome != "survived" else None, pos=o.pos, source=f"{s.name}@{o.tick}", ticks=j - i, timer=timer_running, outcome=outcome))
        i = j


# --- box fit -----------------------------------------------------------------------


def hull(points):
    if not points:
        return None
    lo = [min(p[0][k] for p in points) for k in range(3)]
    hi = [max(p[0][k] for p in points) for k in range(3)]
    return lo, hi


def grow(inner, outside):
    """Grow `inner` on all six faces at once by GROW_STEP; stop a face when the next step
    would take in a point of `outside`, or at GROW_MAX. Returns (lo, hi, limited): the
    box, and per face whether it stopped at GROW_MAX rather than at a point. why not
    unbounded there? a face let go to infinity takes in points far along the other axes
    that the growth never reached, so the box would no longer be outside-free."""
    lo, hi = list(inner[0]), list(inner[1])
    free = [[True, True] for _ in range(3)]
    pts = [p[0] for p in outside]
    for p in pts:
        if all(lo[k] <= p[k] <= hi[k] for k in range(3)):
            raise ValueError(f"inner box already holds an outside point {p}")
    steps = int(GROW_MAX / GROW_STEP)
    for _ in range(steps):
        if not any(f for ff in free for f in ff):
            break
        for k in range(3):
            for side in (0, 1):
                if not free[k][side]:
                    continue
                nlo, nhi = list(lo), list(hi)
                if side == 0:
                    nlo[k] -= GROW_STEP
                else:
                    nhi[k] += GROW_STEP
                if any(all(nlo[m] <= p[m] <= nhi[m] for m in range(3)) for p in pts):
                    free[k][side] = False
                else:
                    lo, hi = nlo, nhi
    return lo, hi, free


def region(name: str, inside, outside, note: str) -> dict:
    inner = hull(inside)
    olo, ohi, limited = grow(inner, outside)
    wlo, whi, faces = [], [], {}
    for k, ax in enumerate("xyz"):
        for side, (iv, ov, w, sign) in enumerate(((inner[0][k], olo[k], wlo, -1), (inner[1][k], ohi[k], whi, 1))):
            face = f"{ax}_{'min' if side == 0 else 'max'}"
            if limited[k][side]:
                w.append(iv + sign * 1.0)
                faces[face] = "assumed"
            else:
                w.append((iv + ov) / 2)
                faces[face] = "fitted"
    status = "fitted" if all(v == "fitted" for v in faces.values()) else "provisional"
    return dict(
        working={"min": wlo, "max": whi},
        inner={"min": inner[0], "max": inner[1]},
        outer={"min": olo, "max": ohi},
        faces=faces, status=status, note=note,
        evidence={"inside": len(inside), "outside": len(outside)},
    )


def near(points, box, margin=1.0):
    lo, hi = box
    return [p for p in points if all(lo[k] - margin <= p[0][k] <= hi[k] + margin for k in range(3))]


# --- main --------------------------------------------------------------------------


START: tuple[float, float, float] = (0.0, 0.0, 0.0)


def read_tape(path: Path):
    header, rows = {}, []
    for line in path.read_text().splitlines():
        data = line.split("#", 1)[0].strip()
        if not data:
            continue
        if "=" in data:
            k, v = data.split("=", 1)
            header[k] = v.strip()
        else:
            rows.append(data.split())
    return header, rows


def git_state() -> tuple[str, bool]:
    try:
        commit = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], capture_output=True, text=True,
                                check=True).stdout.strip()
        dirty = bool(subprocess.run(["git", "-C", str(ROOT), "status", "--porcelain"], capture_output=True,
                                    text=True, check=True).stdout.strip())
        return commit, dirty
    except (OSError, subprocess.CalledProcessError):
        return "unknown", True


def world_header(path: Path) -> dict:
    with gzip.open(path, "rb") as f:
        return json.loads(f.readline())


def main(argv=None):
    global START
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("course", choices=sorted(COURSES))
    ap.add_argument("-o", "--out", required=True)
    ap.add_argument("--world", help="world file (default: the course's)")
    ap.add_argument("--tape", help="tape (default: the course's)")
    args = ap.parse_args(argv)
    cfg = COURSES[args.course]
    out = Path(args.out).resolve()
    tape_path = ROOT / (args.tape or cfg["tape"])
    world_path = (Path(args.world) if args.world else ROOT / cfg["world"]).resolve()
    header, rows = read_tape(tape_path)
    s = header["start"].split()
    unpack = lambda h, f: struct.unpack(f, bytes.fromhex(h))[0]  # noqa: E731
    START = tuple(unpack(h, ">d") for h in s[:3])

    sessions, srcs = [], {"replays": [], "logs": []}
    for name in cfg["replays"]:
        print(f"replay {name}", file=sys.stderr, flush=True)
        sessions.append(replay_session(name, cfg["dimension"]))
        srcs["replays"].append({"name": name, "sha256": sha256_file(REPLAYS / name)})
    for name, rep in cfg["logs"].items():
        print(f"log {name}", file=sys.stderr, flush=True)
        sessions.append(log_session(name, cfg["dimension"]))
        srcs["logs"].append({"name": name, "sha256": sha256_file(LOGS / name), "same_session_as": rep})
    ev = Evidence()
    for sess in sessions:
        collect(sess, ev)

    start_area = region("start_area", ev.start_in, ev.start_out,
                        "Timer starts on the first observed pose outside. Inside: poses judged before each "
                        "timer start, and every pose of attempts that never started it; outside: each timer "
                        "start's latest band pose.")
    timed_finishes(ev, start_area)
    finish = region("finish", ev.finish_in, ev.finish_out,
                    "Finish on the first observed pose inside, timer running. Inside: each finish's latest "
                    "band pose (all from one trajectory: the tape and tas_play replays of it), so inner is "
                    "near a point and the faces the data doesn't reach are assumed; outside: poses judged "
                    "with the timer running before a finish or attempt end.")
    allowed_touch = [t for t in ev.touches if not t["timer"]]
    run_touch = [t for t in ev.touches if t["timer"]]
    kills = [t for t in run_touch if t["outcome"] == "killed"]
    survived = [t for t in run_touch if t["outcome"] == "survived"]
    ground = dict(
        event="any", when={"and": [{"field": "on_ground"}, {"not": {"in": "start_area"}}]},
        after="timer_start", until="finish", status="provisional",
        note=("Conservative: any observed ground touch outside the start area kills. Logged touches with the "
              f"timer running: {len(kills)} killed (reset with no item use, arriving with the glide-off), "
              f"{len(survived)} survived; which touches kill isn't pinned down (single-tick bounces survived "
              "in some places and killed in others; a kill may need the server to have ended the glide). "
              "Touches before the timer start (start area) never killed or started the timer."),
        evidence={"killed": [dict(pos=[round(v, 3) for v in t["pos"]], source=t["source"], ticks=t["ticks"]) for t in kills],
                  "survived": [dict(pos=[round(v, 3) for v in t["pos"]], source=t["source"], ticks=t["ticks"]) for t in survived],
                  "unknown": len(run_touch) - len(kills) - len(survived),
                  "start_area_touches": len(allowed_touch)},
    )
    wh = world_header(world_path)
    commit, dirty = git_state()
    glide_tick = int(header["glide_tick"])
    course = {
        "format": "etas-course", "version": 1, "name": args.course, "dimension": cfg["dimension"],
        "world": {"path": str(world_path.relative_to(out.parent)) if world_path.is_relative_to(out.parent)
                  else str(world_path), "content_sha256": wh["content_sha256"], "extract": cfg["world_extract"]},
        "fitted_from": {**srcs, "excluded": [{"name": k, "why": v} for k, v in cfg["excluded"].items()],
                        "tool": "fit_course.py", "commit": commit, "dirty": dirty,
                        "fitted_at": datetime.datetime.now().astimezone().strftime("%Y-%m-%d %H:%M:%S %Z")},
        "observation": {
            "tick_chat_lag": [2, 3],
            "note": ("Logged sessions (2026-10-20, ~70-85 ms round trip): a tick-end chat on tape tick C judged "
                     "the pose of C-3, or C-2 when the server tick started late; replays without a log are "
                     f"fitted with {list(REPLAY_LAG)}. Kill messages are sent from a packet handler, not at a "
                     "tick end."),
            "sessions": ev.lag,
        },
        "start": {
            "source": {"path": cfg["tape"], "sha256": sha256_file(tape_path)},
            "pose": s[:5],
            "inputs": [[r[1], r[2], r[3]] for r in rows[:glide_tick]],
            "glide_tick": glide_tick, "timer_tick": int(header["timer_tick"]),
            "finish_tick": int(header["finish_tick"]), "finish_text": header["finish_text"],
        },
        "regions": {"start_area": start_area, "finish": finish},
        "rules": {
            "timer_start": dict(event="first", when={"not": {"in": "start_area"}}, after="start",
                                status=start_area["status"], note="see regions.start_area",
                                evidence={"timer_starts": ev.events["timer"], "attempts": ev.events["attempts"]}),
            "finish": dict(event="first", when={"in": "finish"}, after="timer_start", status=finish["status"],
                           note="see regions.finish", evidence={"finishes": ev.events["finish"]}),
            "kill": [dict(name="ground", **ground)],
        },
        "ground_allowed": ["start_area"],
        "boosters": [],
    }
    out.write_text(json.dumps(course, indent=1) + "\n")
    # report
    for name, r in course["regions"].items():
        print(f"{name}: {r['status']}  inside {r['evidence']['inside']}, outside {r['evidence']['outside']}")
        for k in ("inner", "outer", "working"):
            print(f"  {k:8s} min {r[k]['min']}  max {r[k]['max']}")
        print("  faces", r["faces"])
    print(f"events: {ev.events}")
    print(f"ground touches: {len(allowed_touch)} before the timer, timer running: {len(kills)} killed, "
          f"{len(survived)} survived, {len(run_touch) - len(kills) - len(survived)} unknown")
    for t in run_touch:
        print(f"  {t['outcome']:28s} {t['source']:40s} ticks {t['ticks']} pos {tuple(round(v, 2) for v in t['pos'])}")
    for l in ev.lag:
        print(f"lag {l['session']}: {l['note']}")
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
