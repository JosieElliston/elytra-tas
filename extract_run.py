"""Extract a completed course run from a Flashback replay into a TAS input file.

usage: python3 extract_run.py REPLAY.zip [--run N] [-o OUT.tas] [--list]

A run is anchored on the last reset teleport before its "Timer started!" chat (the pose
jumps and comes to rest; that is what the slot-1 "Go Back to Last Checkpoint" item does
with no checkpoint set), or before the finish on a map that sends no such chat, and ends
at the "Course complete!" chat line. A teleport inside the run (a checkpoint reset, a
correction of a move) is written as a server event. The tape starts at the first tick from the
reset on, before the first glide start, that the next tick doesn't move from, on the
ground: at rest, as `start=` promises (the reset tick itself may already move, e.g.
with keys held through it). Inputs are reconstructed per tick:

- yaw/pitch come from the recorded pose, which Flashback samples at end of client
  tick (the mouse only turns the player between ticks, so that is the rotation
  the tick's physics used). Flashback wraps yaw to [-180, 180) and the client's
  yaw is unbounded, so yaw is unwrapped from the reset teleport's absolute yaw.
- every gliding tick is checked against elytra physics (physics.py); ticks that
  don't reproduce are collisions or a model gap, and are listed as warnings.
- movement keys on every tick without the glide (before the deploy, and after a
  lost glide: falling, landing, walking, the redeploy jump) are fitted from the
  horizontal acceleration against vanilla walk physics, from the velocity the
  poses give, the last gliding tick's move, or a server velocity. A tick the model
  can't explain to within FIT_TOL is an error, not a guess. Input slowed to 0.3
  (crouching) is written as sneak on the tick before, which is when
  `LocalPlayer.aiStep` reads it.
- a glide start is a jump press on the tick the glide flag appears; a bounce jump
  (gliding off the ground upward) is a jump press too.
- while gliding keys don't otherwise affect physics; W and sprint are held while the
  sprint flag is set.
- server events the client handled for the player (`server_events`) are written as
  `@server` lines: the server's glide-offs, velocities (boosters, the wall-hit
  echo) and teleports (inferred from the poses). A tape with them is a record of real play, not a plan: replaying it needs
  them (`etas verify` applies them), playing it doesn't send them (`tas_play.pyj`
  skips them), and plan readers refuse it.

The .tas format is line based so the in-game player can parse it without JSON:

    # comment / human-readable columns after the data are ignored
    key=value                     header
    tick yaw pitch keys x y z     one line per tick after the reset
                                  yaw/pitch: float bits (hex), x/y/z: double bits (hex)
                                  keys: subset of "WASDJRC" (R = sprint, C = sneak) or "-"
    @server tick glide_off        the server's glide-off, handled before line tick's
                                  physics: fall flying cleared, the server's pose set
    @server tick velocity x y z   the server's set_entity_motion (double bits): the
                                  velocity replaced before line tick's physics
    @server tick teleport x y z   the server's player_position, which Flashback doesn't
                                  record: x y z is line tick's end pose. Velocity zeroed
                                  before the tick's physics, and the tick ends at x y z
                                  `@server` lines come just before their tick's line,
                                  in the order the client handled them.

`tick` counts from the start tick (0 = the tick whose end pose is `start=`, at rest).
The pose on line t is the expected pose at the end of tick t, after physics.
Header keys besides the obvious: `start_tick=` the replay tick of tick 0 when it is
not `reset_tick=`; `start_flags=` the shared flags at tick 0 in hex when not 0 (0x80
gliding: a reset mid-glide keeps the flag until the server's glide-off; 0x08
sprinting); `glide_tick=` the first glide start; `start_ground=0` tick 0 did not end on
the ground (a tape without it does).

A teleport zeroes the velocity and leaves onGround alone, and the tick's physics runs from
there: held keys already move it, with the ground's speed and friction if the tick before
stood on the ground, and with no downward move it ends with onGround false. So a reset
with keys held never rests. Such a tape starts on the tick before the reset, whose only
use is that onGround and the flags, and its line 1 is the reset, a `@server 1 teleport`.
The velocity a teleport's tick leaves comes from its keys, fitted together with the next
tick's (`fit_reset_keys`).
"""

from __future__ import annotations

import argparse
import math
import struct
import sys
from dataclasses import dataclass
from pathlib import Path

import flashback as fb
import physics

FIT_TOL = 2e-4  # blocks/tick; a correct fit is ~1e-5
GLIDE_TOL = 1e-9  # blocks/tick; a reproduced tick is ~1e-14
TELEPORT_DIST = 2.0  # blocks in one tick that count as a teleport...
REST_DIST = 0.5  # ...when the next tick moves less than this
CRASH_SLACK = 0.3  # blocks/tick an axis can gain in a tick (a jump's 0.42 starts from rest)
TAIL_TICKS = 40  # ticks recorded past the finish, for flying out the finish line


def fbits(v: float) -> str:
    return struct.pack(">f", v).hex()


def dbits(v: float) -> str:
    return struct.pack(">d", v).hex()


@dataclass
class Run:
    reset: int  # tick of reset teleport
    timer: int | None  # tick of "Timer started!"
    finish: int  # tick of "Course complete!"
    finish_text: str
    # start the tape at the last rest before the drop, not the first after the reset.
    # why? what comes before it (walking around the start) isn't timed, and it's where key
    # fits fail: a seed wants the drop. Yaw still unwraps from the reset teleport.
    from_last_rest: bool = False


def find_runs(r: fb.Replay) -> list[Run]:
    poses = filled_poses(r, 0, r.total_ticks)
    motions = {e.tick for e in r.events if e.kind == "motion"}
    teleports = [t for t in range(1, r.total_ticks - 1) if is_teleport(poses, t, motions)]
    runs = []
    last_finish = 0
    for e in r.events:
        if e.kind == "chat" and e.data.startswith("Course complete!"):
            # the attempt that finished is the one its last timer chat started
            timers = [x.tick for x in r.events if x.kind == "chat" and x.data == "Timer started!"
                      and last_finish <= x.tick < e.tick]
            last_finish = e.tick
            limit = timers[-1] if timers else e.tick
            before = [t for t in teleports if t <= limit]
            if not before:
                continue
            runs.append(Run(before[-1], timers[-1] if timers else None, e.tick, e.data))
    return runs


def filled_poses(r: fb.Replay, a: int, b: int) -> dict[int, fb.EntityPos | None]:
    """Pose at end of each tick in [a, b); Flashback omits unchanged ticks."""
    out, cur = {}, None
    for t in range(0, b):
        cur = r.poses.get(t, cur)
        if t >= a:
            out[t] = cur
    return out


def dist(p: fb.EntityPos, q: fb.EntityPos) -> float:
    return math.dist((p.x, p.y, p.z), (q.x, q.y, q.z))


def flag_timeline(r: fb.Replay, b: int) -> dict[int, int]:
    out, cur, by_tick = {}, 0, {}
    for e in r.events:
        if e.kind == "flags":
            by_tick[e.tick] = e.data
    for t in range(b):
        cur = by_tick.get(t, cur)
        out[t] = cur
    return out


# --- walk physics (LivingEntity.travelInAir, MC 26.2), friction-0.6 blocks only ---

GROUND_DRAG = 0.6 * 0.91
AIR_DRAG = 0.91
INPUT_SCALE = 0.98


def predicted_accel(keys: str, yaw: float, on_ground_before: bool, sprinting: bool, jumped: bool,
                    slowly: bool = False):
    fwd = ("W" in keys) - ("S" in keys)
    left = ("A" in keys) - ("D" in keys)
    n = math.hypot(fwd, left)
    if n > 1:
        fwd, left = fwd / n, left / n
    if slowly:  # crouching (LocalPlayer.modifyInput): a ceiling, or the sneak key
        fwd, left = fwd * 0.3, left * 0.3
    if on_ground_before:
        speed = 0.1 * (1.3 if sprinting else 1.0)
    else:
        speed = 0.026 if sprinting else 0.02
    sx, sz = left * INPUT_SCALE * speed, fwd * INPUT_SCALE * speed
    s, c = math.sin(math.radians(yaw)), math.cos(math.radians(yaw))
    ax, az = sx * c - sz * s, sz * c + sx * s
    if jumped and sprinting:
        ax += -s * 0.2
        az += c * 0.2
    return ax, az


KEY_CANDIDATES = ["", "W", "S", "A", "D", "WA", "WD", "SA", "SD"]


def fit_keys(t, poses, flags, reset, v_before=None):
    """Movement keys for tick t (not gliding). Returns (keys, residual, slowly):
    `slowly` is the input scaled by 0.3, crouching, which `LocalPlayer.aiStep`
    decides from the previous tick's sneak key (or a ceiling) before loading this
    tick's keys, so the caller puts the sneak on tick t - 1.

    `v_before` is the velocity tick t starts from when the poses don't give it: a
    server velocity handled before the tick, or the last gliding tick's move (its
    velocity is its move, with no friction after it).
    """
    p0, p1, p2 = poses[t - 2], poses[t - 1], poses[t]
    if t - 1 == reset:
        p0 = p1  # the reset teleport zeroes velocity
    if v_before is None:
        drag = GROUND_DRAG if p0.on_ground else AIR_DRAG
        v_before = (drag * (p1.x - p0.x), 0.0, drag * (p1.z - p0.z))
    ax = (p2.x - p1.x) - v_before[0]
    az = (p2.z - p1.z) - v_before[2]
    sprinting = bool(flags[t] >> fb.FLAG_SPRINT & 1)
    jumped = p1.on_ground and not p2.on_ground and abs((p2.y - p1.y) - 0.42) < 1e-6
    best = None
    for slowly in (False, True):
        for k in KEY_CANDIDATES:
            px, pz = predicted_accel(k, p2.yaw, p1.on_ground, sprinting, jumped, slowly)
            res = math.hypot(ax - px, az - pz)
            if best is None or res < best[1]:
                best = (k, res, slowly)
    keys = best[0] + ("J" if jumped else "") + ("R" if sprinting else "")
    return keys, best[1], best[2]


def fit_reset_keys(t, poses, flags, yaw_t, yaw_next):
    """A teleport at t (velocity zero before its physics): the keys of tick t, from the
    move of tick t + 1, which starts from what tick t's left. Returns (keys of t, the
    velocity it leaves). Gliding: the elytra update from rest, whatever the keys."""
    p, n = poses[t], poses[t + 1]
    if flags[t] >> fb.FLAG_GLIDE & 1:
        return "", tuple(physics.elytra_step((0.0, 0.0, 0.0), yaw_t, p.pitch))
    ground = poses[t - 1].on_ground
    sprinting = bool(flags[t] >> fb.FLAG_SPRINT & 1)
    drag = GROUND_DRAG if ground else AIR_DRAG
    best = None
    for k in KEY_CANDIDATES:
        ax, az = predicted_accel(k, yaw_t, ground, sprinting, False)
        v = (ax * drag, (0.0 - 0.08) * 0.98, az * drag)
        for k2 in KEY_CANDIDATES:
            bx, bz = predicted_accel(k2, yaw_next, p.on_ground, bool(flags[t + 1] >> fb.FLAG_SPRINT & 1),
                                     p.on_ground and not n.on_ground and abs(n.y - p.y - 0.42) < 1e-6)
            res = math.hypot(n.x - p.x - v[0] - bx, n.z - p.z - v[2] - bz)
            if best is None or res < best[0] - 1e-12:
                best = (res, k, v)
    return best[1], best[2]


def is_teleport(poses, t, motions=frozenset()) -> bool:
    """The pose jumps at t and comes to rest after it: a reset or another server
    teleport. why the rest? boosters (2.5 b/t on alpine) and dives move more than
    TELEPORT_DIST in a tick, and their next move is as large. A fast flight that stops
    dead in a wall also moves far and then rests, so a jump the last move explains isn't
    one: every axis no longer than that move, in its direction, up to what one tick can
    add (canyon finishes crash like this, a few ticks before their chat). `motions`, the
    ticks with a server velocity, are where the last move explains nothing."""
    p, q, n = poses.get(t), poses.get(t - 1), poses.get(t + 1)
    if not (p and q and n) or dist(p, q) <= TELEPORT_DIST or dist(n, p) >= REST_DIST:
        return False
    o = poses.get(t - 2)
    if o is None or t in motions:
        return True
    for a, b, c in zip(xyz(o), xyz(q), xyz(p)):
        last, now = b - a, c - b
        if abs(now) > abs(last) + CRASH_SLACK or (now * last < 0 and abs(now) > CRASH_SLACK):
            return True
    return False


SMALL_REST = 0.25  # blocks: the most a move from zero velocity can be (sprint + air control)


def is_correction(poses, flags, t, motions, t0) -> bool:
    """A teleport that doesn't look like a reset, inside a run: the server putting you
    back, by less than TELEPORT_DIST or by nothing at all (zero velocity is all that
    shows). Replay checks can't catch a false one, since a teleport forces the pose, so
    each rule needs the move to be impossible otherwise:
    - the same pose, onGround lost while it stood, and back: a move from zero velocity
      ends in the air, which a standing player's never does;
    - a move against or beyond the last one, followed by a move only zero velocity can
      make. Not a step-up (up to 0.6, on the ground), and not a tick with a server velocity;
    - from rest on the ground, a move no keys give.
    The logs show what these are: `player_position` with the player's own rotation, often
    the pose of the last landing."""
    if t in motions or t - 2 < t0:
        return False
    o, q, p, n = poses.get(t - 2), poses.get(t - 1), poses.get(t), poses.get(t + 1)
    if not (o and q and p and n):
        return False
    if xyz(p) == xyz(q):
        return q.on_ground and not p.on_ground and xyz(n) == xyz(p) and n.on_ground
    if dist(n, p) >= SMALL_REST:
        return False
    d = [b - a for a, b in zip(xyz(q), xyz(p))]
    if xyz(q) == xyz(o) and q.on_ground and p.on_ground and not flags[t] >> fb.FLAG_GLIDE & 1:
        return fit_keys(t, poses, flags, t0)[1] > FIT_TOL
    if q.on_ground and 0 < d[1] <= 0.6:
        return False
    return any(abs(now) > abs(b - a) + CRASH_SLACK or (now * (b - a) < 0 and abs(now) > CRASH_SLACK)
               for now, a, b in zip(d, xyz(o), xyz(q)))


def server_events(r: fb.Replay, flags, a: int, b: int, teleports=()) -> dict[int, list]:
    """What the server did to the local player before each tick's physics in [a, b),
    as tick -> [("glide_off",) | ("velocity", (x, y, z)) | ("teleport", (x, y, z))], in
    handling order (teleports, inferred from the poses, have no order and go last).

    Velocities are the server's set_entity_motion packets (Flashback's own, the last
    of each tick, is already dropped), the LpVec3 decoding the client applied. A
    glide-off is the glide flag going from set to clear between two ticks' flags:
    Flashback drops the server's entity data for the local player, and the client
    never clears the flag itself (a climbable would, which a course doesn't have).
    A glide-off answered by a redeploy press on the tick it arrived leaves the flag
    set at the tick's end and is invisible here; that tick flies like a perfect
    bounce (PLAN-FORMAT.md, @react).
    """
    out: dict[int, list] = {}
    for t in range(a, b):
        if flags[t - 1] >> fb.FLAG_GLIDE & 1 and not flags[t] >> fb.FLAG_GLIDE & 1:
            out.setdefault(t, []).append(("glide_off",))
    for e in r.events:
        if e.kind == "motion" and a <= e.tick < b:
            out.setdefault(e.tick, []).append(("velocity", e.data))
    for t, pos in teleports:
        out.setdefault(t, []).append(("teleport", pos))
    return out


def start_tick(run: Run, poses, flags) -> int:
    """Tick 0 of the tape: the first tick from the reset on, before the first glide,
    whose pose the next tick keeps, on the ground (at rest: the next move was zero, so
    whatever velocity was left is under the zeroing threshold, as a standing start's).
    why not the reset itself? the reset tick's own physics may already move (keys held
    through the teleport, a glide flag kept), and a tape starts from rest at `start=`.
    It is the reset for a run that stands still after it. With `run.from_last_rest`, the
    last such tick before the timer (or the finish), with no glide after it until then."""
    if run.from_last_rest:
        for t in range((run.timer or run.finish) - 1, run.reset, -1):
            p, n = poses[t], poses[t + 1]
            # 0: no glide, and no crouch, which a tape's start can't say
            if p.on_ground and n.on_ground and xyz(p) == xyz(n) and flags[t] & ~(1 << fb.FLAG_SPRINT) == 0:
                return t
    t = run.reset
    while t + 1 < run.finish and not (flags[t + 1] >> fb.FLAG_GLIDE & 1 and not flags[t] >> fb.FLAG_GLIDE & 1):
        p, n = poses[t], poses[t + 1]
        if p.on_ground and n.on_ground and xyz(p) == xyz(n):
            return t
        t += 1
    return run.reset


def extract(r: fb.Replay, run: Run, tail: int = TAIL_TICKS) -> tuple[list[str], list[str], list[str], list]:
    end = min(run.finish + tail, r.total_ticks)
    poses = filled_poses(r, run.reset - 2, end + 1)
    flags = flag_timeline(r, end)
    t0 = start_tick(run, poses, flags)
    moved_start = t0 == run.reset and xyz(poses[t0]) != xyz(poses[t0 + 1])
    if moved_start:
        t0 -= 1
    # Stop the tail at the next teleport (e.g. the post-finish reset) or landing.
    for t in range(run.finish, end):
        landed = not flags[t] >> fb.FLAG_GLIDE & 1
        if landed or is_teleport(poses, t):
            end = t
            break
    lines, problems, warnings = [], [], []
    start = poses[t0]
    ticks = range(t0 + 1, end)
    # unwrapped from the reset teleport's absolute yaw, through any ticks before t0
    since = range(run.reset + 1, end)  # yaw: tick 0's is only a label on a moved start
    yaws = dict(zip(since, physics.unwrap_yaw([poses[t].yaw for t in since], poses[run.reset].yaw)))
    yaws[run.reset] = poses[run.reset].yaw
    yaws[t0] = poses[t0].yaw
    start_yaw = yaws[t0]
    motions = {e.tick for e in r.events if e.kind == "motion"}
    inside = [t for t in range(t0 + 1, run.finish)
              if is_teleport(poses, t, motions) or is_correction(poses, flags, t, motions, t0)]
    if moved_start and t0 + 1 not in inside:
        inside.insert(0, t0 + 1)
    events = server_events(r, flags, t0 + 1, end, [(t, xyz(poses[t])) for t in inside])
    glide_tick = None
    prev_keys = ""
    # where the poses can't give the velocity: a tick after a teleport
    after_teleport = {}
    for t in inside:
        if t + 1 < end:
            after_teleport[t + 1] = fit_reset_keys(t, poses, flags, yaws[t], yaws[t + 1])
    for t in ticks:
        p, p1 = poses[t], poses[t - 1]
        gliding = bool(flags[t] >> fb.FLAG_GLIDE & 1)
        # the flag as tick t's physics started: a glide-off before it clears it
        evs = events.get(t, [])
        was_gliding = bool(flags[t - 1] >> fb.FLAG_GLIDE & 1) and ("glide_off",) not in evs
        v_server = next((e[1] for e in reversed(evs) if e[0] == "velocity"), None)
        if t in after_teleport:
            v_server = after_teleport[t][1]
        # what tick t starts from when the poses don't say: the server's velocity, or
        # the move of a gliding tick before it
        v_before = v_server
        if v_before is None and flags[t - 1] >> fb.FLAG_GLIDE & 1 and t - 1 > t0:
            v_before = tuple(a - b for a, b in zip(xyz(p1), xyz(poses[t - 2])))
        # Keys don't move a gliding player, but the sprint flag is visible to the
        # server; hold W+sprint for as long as the recording kept sprinting.
        glide_keys = "WR" if flags[t] >> fb.FLAG_SPRINT & 1 else ""
        if gliding and not was_gliding:
            # a glide start: a jump press in the air (an edge: no J the tick before)
            glide_tick = t if glide_tick is None else glide_tick
            keys = glide_keys + "J"
            if "J" in prev_keys:
                problems.append(f"tick {t - t0}: glide start right after a jump tick (no edge)")
        elif gliding:
            keys = glide_keys
            # a bounce jump: gliding off the ground upward
            if p1.on_ground and not p.on_ground and p.y > p1.y:
                keys += "J"
        elif t in inside and t + 1 in after_teleport:
            keys = after_teleport[t + 1][0] + ("R" if flags[t] >> fb.FLAG_SPRINT & 1 else "")
        else:
            keys, res, slowly = fit_keys(t, poses, flags, t0, v_before)
            if slowly and lines:
                # sneak on the tick before (a ceiling would do the same without it;
                # the replay in `etas verify` tells them apart)
                lines[-1] = sneak_line(lines[-1])
            if res > FIT_TOL:
                problems.append(f"tick {t - t0}: no key combination fits (residual {res:.2e})")
        if gliding and was_gliding and not p1.on_ground and v_before is not None:
            pred = physics.elytra_step(v_before, yaws[t], p.pitch)
            d = tuple(a - b for a, b in zip(xyz(p), xyz(p1)))
            res = max(abs(a - b) for a, b in zip(d, pred))
            if res > GLIDE_TOL:
                warnings.append((t - t0, res))
        for e in evs:
            if e[0] == "glide_off":
                lines.append(f"@server {t - t0} glide_off")
            else:
                v = e[1]
                lines.append(f"@server {t - t0} {e[0]} {dbits(v[0])} {dbits(v[1])} {dbits(v[2])}"
                             f"    # {v[0]:.6f} {v[1]:.6f} {v[2]:.6f}")
        lines.append(
            f"{t - t0} {fbits(yaws[t])} {fbits(p.pitch)} {keys or '-'} "
            f"{dbits(p.x)} {dbits(p.y)} {dbits(p.z)}"
            f"    # {p.x:.4f} {p.y:.4f} {p.z:.4f} yaw {yaws[t]:.3f} pitch {p.pitch:.3f}"
            f"{' ground' if p.on_ground else ''}"
        )
        prev_keys = keys
    # The pre-glide jump press must be released before the glide press, or the
    # glide jump is not an edge; the fitted keys already have J only on takeoff.
    header = [
        f"source={r.path.name}",
        f"reset_tick={run.reset}",
        f"timer_tick={'' if run.timer is None else run.timer - t0}",
        f"finish_tick={run.finish - t0}",
        f"glide_tick={'' if glide_tick is None else glide_tick - t0}",
        f"finish_text={run.finish_text}",
        f"start={dbits(start.x)} {dbits(start.y)} {dbits(start.z)} {fbits(start_yaw)} {fbits(start.pitch)}"
        f"    # {start.x} {start.y} {start.z} yaw {start_yaw} pitch {start.pitch}",
        f"ticks={sum(1 for x in lines if not x.startswith('@'))}",
    ]
    if t0 != run.reset:
        header.append(f"start_tick={t0}")
    if not start.on_ground:
        header.append("start_ground=0")
    if flags[t0]:
        # a reset doesn't touch the flags: a reset mid-glide keeps gliding until the
        # server's glide-off
        header.append(f"start_flags={flags[t0]:02x}")
    if warnings:
        header.append("glide_mismatch=" + " ".join(f"{t}" for t, _ in warnings))
    return header, lines, problems, warnings


def sneak_line(line: str) -> str:
    """A tick line with C (sneak) added to its keys."""
    f = line.split(" ", 4)
    f[3] = "C" if f[3] == "-" else f[3] + "C"
    return " ".join(f)


def xyz(p):
    return p.x, p.y, p.z


def spans_of(ts):
    spans, a = [], None
    for i, t in enumerate(ts):
        if a is None:
            a = t
        if i + 1 == len(ts) or ts[i + 1] != t + 1:
            spans.append(f"{a}" if a == t else f"{a}-{t}")
            a = None
    return ", ".join(spans)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("replay")
    ap.add_argument("--run", type=int, default=-1, help="index into the run list (default: last)")
    ap.add_argument("--list", action="store_true", help="list completed runs and exit")
    ap.add_argument("-o", "--out", help="output .tas path")
    args = ap.parse_args(argv)

    r = fb.Replay.load(args.replay)
    runs = find_runs(r)
    if args.list or not runs:
        for i, run in enumerate(runs):
            print(f"[{i}] reset@{run.reset} timer@{run.timer} finish@{run.finish}  {run.finish_text}")
        if not runs:
            print("no completed runs found", file=sys.stderr)
            return 1
        return 0
    run = runs[args.run]
    header, lines, problems, warnings = extract(r, run)
    for p in problems:
        print("problem:", p, file=sys.stderr)
    if warnings:
        spans = spans_of([t for t, _ in warnings])
        print(f"warning: {len(warnings)} gliding ticks don't reproduce from rotation alone "
              f"(collisions?): {spans}; worst {max(r for _, r in warnings):.1e}", file=sys.stderr)
    out = Path(args.out) if args.out else Path(Path(args.replay).stem + ".tas")
    out.write_text("\n".join(header + lines) + "\n")
    n_ticks = sum(1 for x in lines if not x.startswith("@"))
    n_ev = len(lines) - n_ticks
    print(f"wrote {out}: {n_ticks} ticks{f', {n_ev} server events' if n_ev else ''}, reset@{run.reset} finish@+{run.finish - run.reset}"
          f"{', ' + str(len(problems)) + ' problems' if problems else ''}")
    return 2 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
