"""Where the timer fired on each flown probe of `/etas queue`: a table for choosing the next.

usage: python3 tools/probe_timer.py QUEUE_DONE_DIR [--names PREFIX] [--rtt-slack MS]

For each answered plan: the log's "Timer started!" and, walking back from it, the moves
by age. EMC's timer message isn't batched at a server tick end like the action bar; on
2026-11-05 it arrives one ping round trip after a move (age 68-71 ms of 34 for a 68 ms
ping, tools/fit_course.py's older band assumed otherwise). The `judged` move is the newest one old enough
to have been answered; `prev` is the move before it. judged is outside the region if the
reply was prompt, and prev is inside if nothing delayed it, so repeats of one plan show how
far to trust that. `bar` is the first action bar's timer value, in ms past the start.
"""
from __future__ import annotations

import argparse
import bisect
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import fit_course as fc
import netlog


def nearby_rtt(log: netlog.Log, t: int) -> float:
    pings = sorted(o.t_send for o in log.outbound if o.name == "ping_request")
    pongs = sorted(i.t_decode for i in log.inbound if i.name == "pong_response")
    k = bisect.bisect_left(pongs, t)
    rtts = []
    for p in pongs[max(0, k - 4):k + 4]:
        j = bisect.bisect_left(pings, p) - 1
        if j >= 0:
            rtts.append(p - pings[j])
    return min(rtts) if rtts else 70e6


def flights(log: netlog.Log, path: Path) -> list[dict]:
    """One dict per attempt in the log's P records: plan, tick and time span."""
    out, plan, cur = [], None, None
    for r in netlog.records(path):
        if r.kind != "P":
            continue
        f = dict(x.split("=", 1) for x in r.f[1:] if "=" in x)
        if r.f[0] == "start":
            plan = f.get("plan")
        elif r.f[0] == "play":
            cur = {"plan": plan, "t0": r.t, "tick0": r.tick}
        elif r.f[0] == "end" and cur:
            cur.update(t1=r.t, line=f.get("line"), result=f.get("result"), reason=f.get("reason"))
            out.append(cur)
            cur = None
    return out


def report(log: netlog.Log, fl: dict, slack: float) -> dict | None:
    moves, last = [], None
    for o in log.outbound:
        m = o.move()
        if m is None:
            continue
        if m["x"] is not None:
            last = (m["x"], m["y"], m["z"])
        if last and fl["t0"] <= o.t_send <= fl["t1"] + 500e6:
            moves.append((o.t_send, o.tick, last))
    chats = [i for i in log.inbound if i.name == "system_chat" and i.details and i.details[-1] == "Timer started!"
             and fl["t0"] <= i.t_decode <= fl["t1"] + 500e6]
    if not chats:
        return None
    c = chats[0]
    rtt = nearby_rtt(log, c.t_decode)
    ts = [m[0] for m in moves]
    j = bisect.bisect_right(ts, c.t_decode - rtt + slack) - 1
    if j < 1:
        return None
    bars = [i for i in log.inbound if i.name == "set_action_bar_text" and i.details and i.t_decode > c.t_decode]
    bar = next((re.match(r"Time: ([\d.]+)", b.details[-1]) for b in bars), None)
    line = moves[j][1] - fl["tick0"]
    return dict(line=line, age=(c.t_decode - moves[j][0]) / 1e6, rtt=rtt / 1e6, judged=moves[j][2], prev=moves[j - 1][2],
                bar=float(bar.group(1)) * 1e3 if bar else None)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("done", type=Path)
    ap.add_argument("--names", default="")
    ap.add_argument("--rtt-slack", type=float, default=3.0)
    a = ap.parse_args(argv)
    answers = [json.loads(p.read_text()) for p in sorted(a.done.glob(f"{a.names}*.json"))]
    want = {r["plan"]: r for r in answers}
    for name in sorted({r["log"] for r in answers if r.get("log")}):
        path = fc.LOGS / name
        log = netlog.load(path)
        for fl in flights(log, path):
            if fl["plan"] not in want:
                continue
            r = report(log, fl, a.rtt_slack * 1e6)
            if r is None:
                print(f"{fl['plan']:<24} no timer  line {fl.get('line')} {fl.get('reason')}")
                continue
            print(f"{fl['plan']:<24} line {r['line']:>4} age {r['age']:5.1f} rtt {r['rtt']:5.1f} "
                  f"judged {r['judged'][0]:8.3f} {r['judged'][1]:8.3f} {r['judged'][2]:8.3f} | "
                  f"prev {r['prev'][0]:8.3f} {r['prev'][1]:8.3f} {r['prev'][2]:8.3f} | bar {r['bar']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
