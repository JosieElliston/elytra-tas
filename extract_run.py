"""Extract a completed course run from a Flashback replay into a TAS input file.

usage: python3 extract_run.py REPLAY.zip [--run N] [-o OUT.tas] [--list]

A run is anchored on a reset teleport (the pose jumps to the course start; that is
what the slot-1 "Go Back to Last Checkpoint" item does with no checkpoint set) and
ends at the "Course complete!" chat line. Inputs are reconstructed per tick:

- yaw/pitch come from the recorded pose, which Flashback samples at end of client
  tick (the mouse only turns the player between ticks, so that is the rotation
  the tick's physics used). Flashback wraps yaw to [-180, 180) and the client's
  yaw is unbounded, so yaw is unwrapped from the reset teleport's absolute yaw.
- every gliding tick is checked against elytra physics (physics.py); ticks that
  don't reproduce are collisions or a model gap, and are listed as warnings.
- movement keys on ground / in air before gliding are fitted from the horizontal
  acceleration against vanilla walk physics. A tick the model can't explain to
  within FIT_TOL is an error, not a guess.
- the glide start is a jump press on the tick the glide flag appears.
- while gliding keys don't affect physics, so none are emitted.

The .tas format is line based so the in-game player can parse it without JSON:

    # comment / human-readable columns after the data are ignored
    key=value                     header
    tick yaw pitch keys x y z     one line per tick after the reset
                                  yaw/pitch: float bits (hex), x/y/z: double bits (hex)
                                  keys: subset of "WASDJR" (R = sprint) or "-"

`tick` counts from the reset tick (0 = first tick the pose is at the start).
The pose on line t is the expected pose at the end of tick t, after physics.
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
TELEPORT_DIST = 2.0  # blocks in one tick that count as a teleport
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


def find_runs(r: fb.Replay) -> list[Run]:
    poses = filled_poses(r, 0, r.total_ticks)
    teleports = [
        t for t in range(1, r.total_ticks)
        if poses[t] and poses[t - 1] and dist(poses[t], poses[t - 1]) > TELEPORT_DIST
    ]
    runs = []
    for e in r.events:
        if e.kind == "chat" and e.data.startswith("Course complete!"):
            before = [t for t in teleports if t < e.tick]
            if not before:
                continue
            reset = before[-1]
            timers = [x.tick for x in r.events
                      if x.kind == "chat" and x.data == "Timer started!" and reset < x.tick < e.tick]
            runs.append(Run(reset, timers[0] if timers else None, e.tick, e.data))
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


def predicted_accel(keys: str, yaw: float, on_ground_before: bool, sprinting: bool, jumped: bool):
    fwd = ("W" in keys) - ("S" in keys)
    left = ("A" in keys) - ("D" in keys)
    n = math.hypot(fwd, left)
    if n > 1:
        fwd, left = fwd / n, left / n
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


def fit_keys(t, poses, flags, reset):
    """Movement keys for tick t (not gliding). Returns (keys, residual)."""
    p0, p1, p2 = poses[t - 2], poses[t - 1], poses[t]
    if t - 1 == reset:
        p0 = p1  # the reset teleport zeroes velocity
    drag = GROUND_DRAG if p0.on_ground else AIR_DRAG
    ax = (p2.x - p1.x) - drag * (p1.x - p0.x)
    az = (p2.z - p1.z) - drag * (p1.z - p0.z)
    sprinting = bool(flags[t] >> fb.FLAG_SPRINT & 1)
    jumped = p1.on_ground and not p2.on_ground and abs((p2.y - p1.y) - 0.42) < 1e-6
    best = None
    for k in KEY_CANDIDATES:
        px, pz = predicted_accel(k, p2.yaw, p1.on_ground, sprinting, jumped)
        res = math.hypot(ax - px, az - pz)
        if best is None or res < best[1]:
            best = (k, res)
    keys = best[0] + ("J" if jumped else "") + ("R" if sprinting else "")
    return keys, best[1]


def extract(r: fb.Replay, run: Run, tail: int = TAIL_TICKS) -> tuple[list[str], list[str], list[str], list]:
    end = min(run.finish + tail, r.total_ticks)
    poses = filled_poses(r, run.reset - 2, end)
    flags = flag_timeline(r, end)
    # Stop the tail at the next teleport (e.g. the post-finish reset) or landing.
    for t in range(run.finish, end):
        landed = not flags[t] >> fb.FLAG_GLIDE & 1
        if landed or dist(poses[t], poses[t - 1]) > TELEPORT_DIST:
            end = t
            break
    lines, problems, warnings = [], [], []
    start = poses[run.reset]
    ticks = range(run.reset + 1, end)
    yaws = dict(zip(ticks, physics.unwrap_yaw([poses[t].yaw for t in ticks], start.yaw)))
    glide_tick = None
    for t in ticks:
        p = poses[t]
        gliding = bool(flags[t] >> fb.FLAG_GLIDE & 1)
        was_gliding = bool(flags[t - 1] >> fb.FLAG_GLIDE & 1)
        # Keys don't move a gliding player, but the sprint flag is visible to the
        # server; hold W+sprint for as long as the recording kept sprinting.
        glide_keys = "WR" if flags[t] >> fb.FLAG_SPRINT & 1 else ""
        if gliding and not was_gliding:
            if glide_tick is not None:
                problems.append(f"tick {t - run.reset}: second glide start (ebounce?) not supported yet")
            glide_tick = t
            keys = glide_keys + "J"
        elif gliding:
            keys = glide_keys
        elif was_gliding:
            problems.append(f"tick {t - run.reset}: glide ended mid-run, not supported yet")
            keys = ""
        else:
            keys, res = fit_keys(t, poses, flags, run.reset)
            if res > FIT_TOL:
                problems.append(f"tick {t - run.reset}: no key combination fits (residual {res:.2e})")
        if gliding and glide_tick is not None and t >= glide_tick + 2:
            v = tuple(a - b for a, b in zip(xyz(poses[t - 1]), xyz(poses[t - 2])))
            pred = physics.elytra_step(v, yaws[t], p.pitch)
            d = tuple(a - b for a, b in zip(xyz(p), xyz(poses[t - 1])))
            res = max(abs(a - b) for a, b in zip(d, pred))
            if res > GLIDE_TOL:
                warnings.append((t - run.reset, res))
        lines.append(
            f"{t - run.reset} {fbits(yaws[t])} {fbits(p.pitch)} {keys or '-'} "
            f"{dbits(p.x)} {dbits(p.y)} {dbits(p.z)}"
            f"    # {p.x:.4f} {p.y:.4f} {p.z:.4f} yaw {yaws[t]:.3f} pitch {p.pitch:.3f}"
            f"{' ground' if p.on_ground else ''}"
        )
    # The pre-glide jump press must be released before the glide press, or the
    # glide jump is not an edge; the fitted keys already have J only on takeoff.
    header = [
        f"source={r.path.name}",
        f"reset_tick={run.reset}",
        f"timer_tick={'' if run.timer is None else run.timer - run.reset}",
        f"finish_tick={run.finish - run.reset}",
        f"glide_tick={'' if glide_tick is None else glide_tick - run.reset}",
        f"finish_text={run.finish_text}",
        f"start={dbits(start.x)} {dbits(start.y)} {dbits(start.z)} {fbits(start.yaw)} {fbits(start.pitch)}"
        f"    # {start.x} {start.y} {start.z} yaw {start.yaw} pitch {start.pitch}",
        f"ticks={len(lines)}",
    ]
    if warnings:
        header.append("glide_mismatch=" + " ".join(f"{t}" for t, _ in warnings))
    return header, lines, problems, warnings


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
    print(f"wrote {out}: {len(lines)} ticks, reset@{run.reset} finish@+{run.finish - run.reset}"
          f"{', ' + str(len(problems)) + ' problems' if problems else ''}")
    return 2 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
