r"""Do replies to a move wait for the server's tick work? Timer chats and probes against tick phase.

usage: python3 tools/tick_wait.py LOG.tsv.gz... [--bin MS] [--ping] [--since SECONDS]

S is how long before the next tick-end flush a packet reaches the server, in ms [0, 50): the
flush phase comes from set_health (EMC sends it every tick, flushed at the tick's end), and a
packet's arrival is compared in round-trip terms, so how the ping splits up and down doesn't
matter. L is how late its reply is, past one round trip. Tab-completion probes are answered on
the main thread between ticks (docs/tick-phase.md), so for them L follows S across the tick
work and sits near 1 ms past it. A reply handled off the main thread would stay flat.

D says when the server read its start clock. The action bar gives the timer in ms, so a bar
decoded at time b showing v puts the reading at b - v, plus the fixed gap from reading the bar's
clock to the flush. D is that minus the move's arrival, the 10th percentile over a flight's bars,
because late packets only push it up. It rises with the wait if the clock reads after the wait.

`--runs` lists finished runs: shown time, send gap, chat gap, and each end's S and L.

Needs the mod's pings, so only logs from whitelisted sessions. `--ping` prints round trip, tick
spacing and move spacing per 15 s, for spotting a ping that moved during a batch; flights with a
moving ping can't be placed in the tick.
"""
from __future__ import annotations

import argparse
import bisect
import re
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).parent))
import netlog

TICK_NS = 50_000_000
TIMER_CHAT = "Timer started!"


def flush_phase(health: list[int], t: float) -> float | None:
    """Tick-end flush phase near t, ms mod 50: the densest 4 ms of set_health within 3 s.

    why the densest stretch and not a mean? about 30% of downstream packets come 10-90 ms late
    in clumps, and only their on-time edge marks the flush.
    """
    i, j = bisect.bisect_left(health, t - 3e9), bisect.bisect_right(health, t + 3e9)
    if j - i < 40:
        return None
    a = (np.array(health[i:j], dtype=np.int64) % TICK_NS) / 1e6
    h, _ = np.histogram(a, bins=50, range=(0, 50))
    c = int(np.argmax(np.convolve(np.r_[h, h[:3]], np.ones(4), "valid")[:50]))
    d = np.mod(a - c + 25, 50) - 25
    return (c + float(np.median(d[(d >= 0) & (d < 4)]))) % 50


class Session:
    def __init__(self, path: Path):
        self.path = path
        self.log = log = netlog.load(path)
        self.health = sorted(i.t_decode for i in log.inbound if i.name == "set_health")
        pings = sorted(o.t_wire or o.t_send for o in log.outbound if o.name == "ping_request")
        self.rtts: list[tuple[int, int]] = []
        for p in sorted(i.t_decode for i in log.inbound if i.name == "pong_response"):
            j = bisect.bisect_left(pings, p) - 1
            if j >= 0:
                self.rtts.append((p, p - pings[j]))
        self.rtt_t = [r[0] for r in self.rtts]
        self.moves = sorted(o.t_wire or o.t_send for o in log.outbound if o.move() is not None)

    def rtt(self, t: int) -> int | None:
        """The fastest of the 16 nearest pings; a late pong says nothing about the path."""
        k = bisect.bisect_left(self.rtt_t, t)
        near = [r[1] for r in self.rtts[max(0, k - 8):k + 8]]
        return min(near) if near else None

    def place(self, sent: int, reply: int) -> tuple[float, float, float] | None:
        """(S, L, rtt) in ms for a packet sent at `sent` and answered at `reply`."""
        r = self.rtt(reply)
        p = flush_phase(self.health, reply)
        if r is None or p is None:
            return None
        arrival = sent + r
        return (p - (arrival % TICK_NS) / 1e6) % 50, (reply - arrival) / 1e6, r / 1e6

    def chats(self) -> list[tuple[float, float, float]]:
        out = []
        for i in self.log.inbound:
            if i.name != "system_chat" or not i.details or i.details[-1] != TIMER_CHAT:
                continue
            r = self.rtt(i.t_decode)
            if r is None:
                continue
            # the newest move old enough to be answered, 3 ms slack for a fast round trip
            j = bisect.bisect_right(self.moves, i.t_decode - r + 3e6) - 1
            if j >= 0 and (x := self.place(self.moves[j], i.t_decode)):
                out.append(x)
        return out

    def clock_reads(self) -> list[tuple[float, float, float]]:
        """(S, L, D) per timer chat with at least 5 bars before the timer's next reset."""
        bars = [(i.t_decode, float(m.group(1))) for i in self.log.inbound if i.name == "set_action_bar_text"
                and i.details and (m := re.match(r"Time: ([\d.]+)", i.details[-1]))]
        at = [b[0] for b in bars]
        out = []
        for c in self.log.inbound:
            if c.name != "system_chat" or not c.details or c.details[-1] != TIMER_CHAT:
                continue
            r = self.rtt(c.t_decode)
            if r is None:
                continue
            j = bisect.bisect_right(self.moves, c.t_decode - r + 3e6) - 1
            if j < 0 or not (x := self.place(self.moves[j], c.t_decode)):
                continue
            d = []
            for b, v in bars[bisect.bisect_left(at, c.t_decode - 5e6):]:
                # a value that doesn't fit this chat's clock belongs to the next attempt
                if b - c.t_decode > 4e9 or v * 1e9 > b - c.t_decode + 2e9:
                    break
                d.append((b - v * 1e9 - self.moves[j] - r) / 1e6)
            if len(d) >= 5:
                out.append((x[0], x[1], float(np.percentile(d, 10))))
        return out

    def probes(self) -> list[tuple[float, float, float]]:
        sent = sorted(o.t_wire or o.t_send for o in self.log.outbound if o.name == "command_suggestion")
        out = []
        for a in self.log.inbound:
            if a.name != "command_suggestions":
                continue
            j = bisect.bisect_left(sent, a.t_decode) - 1
            if j >= 0 and (x := self.place(sent[j], a.t_decode)) and -5 < x[1] < 120:
                out.append(x)
        return out

    def move_phase(self, since_s: float) -> np.ndarray:
        """S of the moves in flights and other steady flying; not a standing player's 1 Hz moves."""
        t0 = self.log.t0 + since_s * 1e9
        out = []
        for a, b in zip(self.moves, self.moves[1:]):
            if a >= t0 and b - a < 100e6 and (r := self.rtt(a)) and (p := flush_phase(self.health, a)) is not None:
                out.append((p - ((a + r) % TICK_NS) / 1e6) % 50)
        return np.array(out)


def table(rows: list[tuple[float, float, float]], width: int) -> None:
    if not rows:
        print("  none")
        return
    s, late = np.array([r[0] for r in rows]), np.array([r[1] for r in rows])
    for b in range(0, 50, width):
        m = (s >= b) & (s < b + width)
        if m.any():
            print(f"  S {b:2d}-{b + width:2d}  n={m.sum():5d}  L med {np.median(late[m]):6.1f}"
                  f"  p10 {np.percentile(late[m], 10):6.1f}  p90 {np.percentile(late[m], 90):6.1f}"
                  f"  >10 ms {np.mean(late[m] > 10):.2f}")


def run_lines(sn: Session) -> None:
    inb = sn.log.inbound
    timers = [i for i in inb if i.name == "system_chat" and i.details and i.details[-1] == TIMER_CHAT]
    at = [t.t_decode for t in timers]
    for c in inb:
        m = c.name == "system_chat" and c.details and re.match(r"Course complete! Your time: ([\d.]+)", c.details[-1])
        k = bisect.bisect_left(at, c.t_decode) - 1
        if not m or k < 0:
            continue
        ends = []
        for chat in (timers[k], c):
            r = sn.rtt(chat.t_decode)
            j = bisect.bisect_right(sn.moves, chat.t_decode - r + 3e6) - 1 if r else -1
            ends.append((j, sn.place(sn.moves[j], chat.t_decode)) if j >= 0 else (j, None))
        (js, xs), (jf, xf) = ends
        if not xs or not xf:
            continue
        send = (sn.moves[jf] - sn.moves[js]) / 1e6
        print(f"  shown {float(m.group(1)) * 1000:.0f}  moves {jf - js}  send gap {send:.1f}"
              f"  chat gap {(c.t_decode - timers[k].t_decode) / 1e6:.1f}"
              f"  start S {xs[0]:.1f} L {xs[1]:.1f}  finish S {xf[0]:.1f} L {xf[1]:.1f}")


def ping_lines(sn: Session) -> None:
    t0, end = sn.log.t0, max(sn.rtt_t)
    for b in np.arange(t0, end, 15e9):
        r = [x / 1e6 for t, x in sn.rtts if b <= t < b + 15e9]
        if not r:
            continue
        h = np.diff([x for x in sn.health if b <= x < b + 15e9]) / 1e6
        g = np.diff([x for x in sn.moves if b <= x < b + 15e9]) / 1e6
        tick = f"{np.percentile(h, 10):5.1f} {np.percentile(h, 90):5.1f}" if len(h) else "  -     - "
        gap = f"{np.percentile(g, 10):6.1f} {np.percentile(g, 90):6.1f}" if len(g) else "   -      - "
        print(f"  {(b - t0) / 1e9:5.0f} s  rtt min {min(r):6.1f} med {np.median(r):6.1f} max {max(r):6.1f}"
              f"  health gap p10/p90 {tick}  moves {len(g) + 1:3d} gap p10/p90 {gap}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("logs", nargs="+", type=Path)
    ap.add_argument("--bin", type=int, default=3, help="width of an S bin in ms")
    ap.add_argument("--ping", action="store_true", help="round trip and spacing per 15 s")
    ap.add_argument("--runs", action="store_true", help="finished runs, one per line")
    ap.add_argument("--since", type=float, default=0, help="only moves this many seconds into the log")
    a = ap.parse_args()
    chats, probes, clocks = [], [], []
    for path in a.logs:
        sn = Session(path)
        print(f"{path.name}  {sn.log.meta.get('player')}  mod {sn.log.meta.get('mod')}")
        if not sn.rtts or len(sn.health) < 100:
            print("  no pings or set_health; skipped")
            continue
        if a.ping:
            ping_lines(sn)
        if a.runs:
            run_lines(sn)
        ms = sn.move_phase(a.since)
        if len(ms):
            print(f"  moves in steady flying: {len(ms)}, S hist per 5 ms: {np.histogram(ms, bins=10, range=(0, 50))[0]}")
            # circular, so a band straddling the flush doesn't average to the far side of the tick
            c = np.angle(np.mean(np.exp(2j * np.pi * ms / 50))) * 50 / (2 * np.pi) % 50
            d = np.mod(ms - c + 25, 50) - 25
            print(f"  S center {c:.1f}, p5 {c + np.percentile(d, 5):.1f}, p95 {c + np.percentile(d, 95):.1f}")
        chats += sn.chats()
        clocks += sn.clock_reads()
        probes += sn.probes()
    print("timer chats")
    table(chats, a.bin)
    print("probes")
    table(probes, a.bin)
    print("start clock read: S, L, D")
    for x in sorted(clocks):
        print(f"  {x[0]:5.1f} {x[1]:5.1f} {x[2]:5.1f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
