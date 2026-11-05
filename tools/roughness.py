#!/usr/bin/env python3
"""How rough a trajectory's rotation is, to compare two builds of a plan.

A tick's roughness is how far its rate departs from the mean of its neighbors' rates, degrees per
tick, the larger of pitch and yaw. Yaw is weighted by cos(pitch), since yaw barely steers near
vertical. It flags sharp changes, not whether they are needed: a wall can ask for one.

    python3 tools/roughness.py [--top N] [--from T] [--to T] FILE.traj.jsonl ...
"""
import argparse
import json
import math


def load(path):
    with open(path) as f:
        rows = [json.loads(line) for line in f.read().splitlines()[3:]]
    # unwrapped, so a turn through +-180 isn't a jump
    yaw = []
    for r in rows:
        y = r["input"]["yaw"]
        if yaw:
            y += 360 * round((yaw[-1] - y) / 360)
        yaw.append(y)
    return rows, yaw, [r["input"]["pitch"] for r in rows]


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("files", nargs="+")
    ap.add_argument("--top", type=int, default=12, help="the roughest ticks to list")
    ap.add_argument("--from", dest="lo", type=int, default=0, help="first tick counted")
    ap.add_argument("--to", dest="hi", type=int, default=10**9, help="last tick counted")
    a = ap.parse_args()
    for path in a.files:
        rows, yaw, pitch = load(path)
        n = len(rows)
        dy = [0.0] + [yaw[k] - yaw[k - 1] for k in range(1, n)]
        dp = [0.0] + [pitch[k] - pitch[k - 1] for k in range(1, n)]
        ticks = [
            k
            for k in range(1, n - 1)
            if rows[k]["state"]["fall_flying"] and a.lo <= rows[k]["t"] <= a.hi
        ]
        rough = {}
        for k in ticks:
            c = math.cos(math.radians(pitch[k]))
            rough[k] = max(
                abs(dp[k] - (dp[k - 1] + dp[k + 1]) / 2),
                abs(dy[k] - (dy[k - 1] + dy[k + 1]) / 2) * c,
            )
        flips = sum(
            1
            for k in ticks
            if dp[k] * dp[k + 1] < 0 and min(abs(dp[k]), abs(dp[k + 1])) >= 0.3
        )
        flips_y = sum(
            1
            for k in ticks
            if dy[k] * dy[k + 1] < 0
            and min(abs(dy[k]), abs(dy[k + 1])) * math.cos(math.radians(pitch[k])) >= 0.3
        )
        print(path)
        print(
            f"  {len(ticks)} ticks; travel pitch {sum(abs(dp[k]) for k in ticks):.0f} "
            f"yaw {sum(abs(dy[k]) for k in ticks):.0f} deg; "
            f"rough sum {sum(rough.values()):.0f}; "
            f"rough >=0.7: {sum(r >= 0.7 for r in rough.values())}, "
            f">=2: {sum(r >= 2 for r in rough.values())}, "
            f">=10: {sum(r >= 10 for r in rough.values())}; "
            f"reversals pitch {flips} yaw {flips_y}"
        )
        for k in sorted(rough, key=rough.get, reverse=True)[: a.top]:
            x, y, z = rows[k]["state"]["position"]
            print(
                f"  t {rows[k]['t']:4d} ({x:7.1f} {y:6.1f} {z:7.1f}) rough {rough[k]:6.2f}  "
                f"pitch {pitch[k]:7.2f} d {dp[k]:7.2f}  yaw d {dy[k]:8.2f}"
            )


if __name__ == "__main__":
    main()
